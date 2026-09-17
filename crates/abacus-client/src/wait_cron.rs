//! WaitCron: a recurring grid-aligned timer. The daemon auto-re-arms after
//! each wake to the next grid line. The client loops futex_wait without any
//! UDS round-trip per cycle.

use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wait, futex_word, monotonic_now_nanos};
use abacus_core::interlock::{interlock_free, InterlockHandle, SENTINEL};

use crate::client::SdkError;
use crate::touch::TouchThread;
use crate::types::{WaitResult, WaitState, DEFAULT_TIMEOUT_NANOS, DEFAULT_TOUCH_INTERVAL_MS};

/// A WaitCron interlock. Grid-aligned to the monotonic epoch. The daemon
/// auto-re-arms open_count after each wake, so the client just loops wait().
pub struct WaitCron {
    handle: InterlockHandle,
    touch_thread: Option<TouchThread>,
}

impl WaitCron {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        let touch_thread = Some(TouchThread::spawn(handle.clone(), DEFAULT_TOUCH_INTERVAL_MS));
        Self {
            handle,
            touch_thread,
        }
    }

    /// Block until the next grid line fires. The daemon stamps closed_count
    /// and auto-re-arms open_count. Call wait() again to sleep until the
    /// next firing.
    ///
    /// Returns WaitResult with completed_at = the clock value when the daemon
    /// delivered. Normal if on time, Overrun if the daemon was late.
    ///
    /// The check: snapshot closed_count at entry, then wait for it to advance.
    /// After the daemon fires, it stamps closed_count = clock_now_ms and
    /// re-arms open_count to the next grid line. So open > closed is the
    /// expected steady state. We detect firing by closed_count advancing
    /// past the snapshot, not by comparing closed vs open.
    pub fn wait(&self) -> Result<WaitResult, SdkError> {
        let words = self.handle.words();
        let closed_word = &words.closed_count;

        // Snapshot the current closed_count.
        let initial_closed = closed_word.load(Ordering::Acquire);

        loop {
            let closed = closed_word.load(Ordering::Acquire);
            if closed == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if closed > initial_closed {
                // Daemon fired and stamped a new closed_count.
                let open = words.open_count.load(Ordering::Acquire);
                let state = if closed >= open {
                    // closed >= open means daemon was late (overrun) or
                    // exactly on time before re-arm propagated.
                    if closed == open {
                        WaitState::Normal
                    } else {
                        WaitState::Overrun
                    }
                } else {
                    // open > closed: daemon re-armed to next grid line.
                    // This is the normal post-fire state.
                    WaitState::Normal
                };
                return Ok(WaitResult {
                    completed_at: closed,
                    state,
                });
            }

            let exp = words.expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }

            let lo32 = futex_word(closed);
            futex_wait(closed_word, lo32, DEFAULT_TIMEOUT_NANOS);
        }
    }

    /// Read the daemon's response (closed_count): the clock value at last wake.
    pub fn completed_at(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
    }

    /// Read both counters: (open_count, closed_count).
    /// open_count = next grid line target, closed_count = last wake time.
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

impl Drop for WaitCron {
    fn drop(&mut self) {
        self.touch_thread.take();
    }
}
