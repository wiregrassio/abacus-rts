//! WaitCounter: watches another interlock's counter. The daemon stamps
//! closed_count when the watched value crosses the target.

use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wait, futex_word, ms_to_nanos};
use abacus_core::interlock::{interlock_arm, interlock_free, InterlockHandle, SENTINEL};

use crate::client::SdkError;
use crate::touch::TouchThread;
use crate::types::{check_wake_state, WaitResult, WaitState, DEFAULT_TOUCH_INTERVAL_MS};

/// A WaitCounter interlock. The client writes open_count (the target) and
/// futex-waits on closed_count (the daemon's response).
pub struct WaitCounter {
    handle: InterlockHandle,
    touch_thread: Option<TouchThread>,
}

impl WaitCounter {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        let touch_thread = Some(TouchThread::spawn(handle.clone(), DEFAULT_TOUCH_INTERVAL_MS));
        Self {
            handle,
            touch_thread,
        }
    }

    /// Set open_count to `target`, extend expiration, and futex-wait on
    /// closed_count until the daemon delivers.
    ///
    /// `timeout_ms` is the futex timeout per iteration in milliseconds. TTL is
    /// set to 2 * timeout_ms. For most uses pass 100; for long waits pass a
    /// larger value (e.g. 5000).
    ///
    /// Returns a WaitResult with the daemon's response and the wake state.
    /// For WaitCounter, Timeout handling is the caller's decision.
    /// Returns Err(InterlockReaped) if the interlock is reaped before delivery.
    pub fn wait_until(&self, target: u64, timeout_ms: u64) -> Result<WaitResult, SdkError> {
        let words = self.handle.words();
        let timeout_nanos = ms_to_nanos(timeout_ms);

        // CAS-max: never writes open_count backward. A target below the current
        // value is a no-op. Sequential wait_until calls are monotonic.
        let open = &words.open_count;
        loop {
            let current = open.load(Ordering::Acquire);
            if target <= current {
                break;
            }
            match open.compare_exchange_weak(current, target, Ordering::Release, Ordering::Acquire) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }

        // Extend expiration (2x the timeout as TTL margin).
        // If the interlock is already reaped, surface it immediately.
        let ttl = timeout_nanos.saturating_mul(2);
        interlock_arm(&self.handle, ttl).map_err(|_| SdkError::InterlockReaped)?;

        // 3. Futex-wait loop on closed_count.
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
                    // open > closed: daemon has not delivered yet. Wait.
                    // Uses the low 32 bits of the counter for futex comparison
                    // (Linux kernel constraint). An increment that is an exact
                    // multiple of 2^32 landing in the load-to-syscall window adds
                    // one timeout cycle of latency. No realistic increment
                    // triggers this.
                    let lo32 = futex_word(closed);
                    let ret = futex_wait(closed_word, lo32, timeout_nanos);

                    // After wake or timeout, re-check.
                    let closed_now = closed_word.load(Ordering::Acquire);
                    let open_now = words.open_count.load(Ordering::Acquire);
                    let state_now = check_wake_state(open_now, closed_now);

                    match state_now {
                        WaitState::Normal | WaitState::Overrun => {
                            return Ok(WaitResult {
                                completed_at: closed_now,
                                state: state_now,
                            });
                        }
                        WaitState::Timeout => {
                            let exp = words.expiration_ns.load(Ordering::Acquire);
                            if exp == SENTINEL {
                                return Err(SdkError::InterlockReaped);
                            }
                            // Check if the futex actually timed out (ret == -1 with ETIMEDOUT).
                            if ret == -1 {
                                let errno = std::io::Error::last_os_error()
                                    .raw_os_error()
                                    .unwrap_or(0);
                                if errno == libc::ETIMEDOUT {
                                    return Ok(WaitResult {
                                        completed_at: closed_now,
                                        state: WaitState::Timeout,
                                    });
                                }
                            }
                            // Spurious wake or EINTR/EAGAIN: loop again.
                            continue;
                        }
                    }
                }
            }
        }
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

    /// Signed difference: open_count - closed_count.
    ///
    /// Wrapping i64 subtraction. Counters above i64::MAX (2^63) are outside the
    /// spec's representable range. No defined tier reaches this.
    pub fn value(&self) -> i64 {
        let (open, closed) = self.peek();
        (open as i64).wrapping_sub(closed as i64)
    }

    pub fn free(&mut self) {
        self.touch_thread.take();
        interlock_free(&self.handle);
    }
}

impl Drop for WaitCounter {
    fn drop(&mut self) {
        self.touch_thread.take();
    }
}
