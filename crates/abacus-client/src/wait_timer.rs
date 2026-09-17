//! WaitTimer: a WaitCounter that watches clock.open_count. SDK sugar for
//! time-based waits. Timeout is fatal (RTSTimeout).

use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wait, futex_word, monotonic_now_nanos, ms_to_nanos};
use abacus_core::interlock::{interlock_arm, interlock_free, InterlockHandle, SENTINEL};

use crate::client::SdkError;
use crate::touch::TouchThread;
use crate::types::{check_wake_state, WaitResult, WaitState, DEFAULT_TOUCH_INTERVAL_MS};

/// A WaitTimer interlock. Watches clock.open_count. wait_ms sets a target
/// relative to the current clock and futex-waits for the daemon to deliver.
/// Timeout is fatal: the SDK aborts the process (RTSTimeout).
pub struct WaitTimer {
    handle: InterlockHandle,
    clock: InterlockHandle,
    touch_thread: Option<TouchThread>,
}

impl WaitTimer {
    pub(crate) fn new(handle: InterlockHandle, clock: InterlockHandle) -> Self {
        let touch_thread = Some(TouchThread::spawn(handle.clone(), DEFAULT_TOUCH_INTERVAL_MS));
        Self {
            handle,
            clock,
            touch_thread,
        }
    }

    /// Wait for `ms` milliseconds from the current clock time.
    ///
    /// Sets open_count = clock_now + ms, extends expiration to 2 * ms, and
    /// futex-waits on closed_count with an absolute deadline of 2 * ms from now.
    ///
    /// Absolute deadline instead of per-iteration timeout. Each EINTR or spurious
    /// return subtracts from the remaining budget instead of restarting a fresh
    /// 2*W. This ensures RTSTimeout fires within 2*W wall-clock time regardless
    /// of signal delivery.
    ///
    /// On Normal or Overrun wake: returns WaitResult.
    /// On Timeout: RTSTimeout. Fatal. Aborts the process.
    /// Returns Err(InterlockReaped) if the interlock is reaped before delivery.
    pub fn wait_ms(&self, ms: u64) -> Result<WaitResult, SdkError> {
        let words = self.handle.words();

        // 1. Read current clock time (open_count = current monotonic ms).
        let clock_now = self.clock.words().open_count.load(Ordering::Acquire);

        // 2. CAS-max: never writes open_count backward. Concurrent wait_ms calls
        // with different durations preserve the largest target.
        let target = clock_now + ms;
        loop {
            let current = words.open_count.load(Ordering::Acquire);
            if target <= current {
                break;
            }
            match words.open_count.compare_exchange_weak(
                current,
                target,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }

        // 3. Extend expiration to max(current, now + 2 * ms).
        //    If the interlock is already reaped, surface it immediately.
        let ttl_nanos = ms_to_nanos(ms.saturating_mul(2));
        interlock_arm(&self.handle, ttl_nanos).map_err(|_| SdkError::InterlockReaped)?;

        // 4. Compute absolute deadline: now + 2 * ms in CLOCK_MONOTONIC nanos.
        let deadline_ns = monotonic_now_nanos().saturating_add(ttl_nanos);

        // 5. Futex-wait loop on closed_count with shrinking remaining budget.
        let closed_word = &words.closed_count;
        loop {
            let closed = closed_word.load(Ordering::Acquire);
            let open = words.open_count.load(Ordering::Acquire);
            let state = check_wake_state(open, closed);

            match state {
                WaitState::Normal | WaitState::Overrun => {
                    return Ok(WaitResult {
                        completed_at: closed,
                        state,
                    });
                }
                WaitState::Timeout => {
                    let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
                    if exp == SENTINEL {
                        return Err(SdkError::InterlockReaped);
                    }
                    // Compute remaining budget from absolute deadline.
                    let now_ns = monotonic_now_nanos();
                    if now_ns >= deadline_ns {
                        // Check reaped one more time (expiration could have
                        // lapsed naturally within the budget window).
                        if exp < now_ns {
                            return Err(SdkError::InterlockReaped);
                        }
                        // RTSTimeout: wall-clock budget exhausted. Fatal.
                        eprintln!(
                            "abacus: RTSTimeout: daemon did not deliver within {}ms \
                             (target={}, open={}, closed={}); aborting",
                            ms * 2,
                            target,
                            open,
                            closed,
                        );
                        std::process::abort();
                    }
                    let remaining_ns = deadline_ns - now_ns;

                    // Uses the low 32 bits of the counter for futex comparison
                    // (Linux kernel constraint). An increment that is an exact
                    // multiple of 2^32 landing in the load-to-syscall window
                    // adds one timeout cycle of latency. No realistic increment
                    // triggers this.
                    let lo32 = futex_word(closed);
                    futex_wait(closed_word, lo32, remaining_ns);

                    // Re-check after wake. The loop re-evaluates state and
                    // remaining budget on the next iteration.
                }
            }
        }
    }

    /// Wait until an absolute clock timestamp. Sugar for
    /// wait_ms(timestamp_ms - clock.open_count), saturating to zero if the
    /// timestamp is already past.
    pub fn wait_until(&self, timestamp_ms: u64) -> Result<WaitResult, SdkError> {
        let clock_now = self.clock.words().open_count.load(Ordering::Acquire);
        let ms = timestamp_ms.saturating_sub(clock_now);
        self.wait_ms(ms)
    }

    /// Read the daemon's response (closed_count).
    pub fn completed_at(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
    }

    /// Extend expiration to max(current, now + ms).
    /// Returns Err(InterlockReaped) if the interlock has been reaped.
    pub fn touch(&self, ms: u64) -> Result<(), SdkError> {
        interlock_arm(&self.handle, ms_to_nanos(ms)).map_err(|_| SdkError::InterlockReaped)
    }

    /// Read both counters: (open_count, closed_count).
    pub fn peek(&self) -> (u64, u64) {
        let words = self.handle.words();
        let open = words.open_count.load(Ordering::Acquire);
        let closed = words.closed_count.load(Ordering::Acquire);
        (open, closed)
    }

    pub fn free(&mut self) {
        self.touch_thread.take();
        interlock_free(&self.handle);
    }
}

impl Drop for WaitTimer {
    fn drop(&mut self) {
        self.touch_thread.take();
    }
}
