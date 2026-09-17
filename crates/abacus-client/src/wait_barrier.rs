//! WaitBarrier: wait for all of N conditions to be met. Daemon-evaluated:
//! one interlock, one check per cycle for all N conditions. Fires when all
//! watched interlocks cross their respective thresholds.

use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wait, futex_word, monotonic_now_nanos};
use abacus_core::interlock::{interlock_free, InterlockHandle, SENTINEL};

use crate::client::SdkError;
use crate::touch::TouchThread;
use crate::types::{WaitResult, WaitState, DEFAULT_TIMEOUT_NANOS, DEFAULT_TOUCH_INTERVAL_MS};

/// A WaitBarrier interlock. The daemon checks all conditions each cycle and
/// stamps closed_count = clock.open_count when all are met.
pub struct WaitBarrier {
    handle: InterlockHandle,
    touch_thread: Option<TouchThread>,
}

impl WaitBarrier {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        // The daemon sets open_count = 1 at creation to make the barrier Open
        // so it evaluates conditions each cycle.
        let touch_thread = Some(TouchThread::spawn(handle.clone(), DEFAULT_TOUCH_INTERVAL_MS));
        Self {
            handle,
            touch_thread,
        }
    }

    /// Block until all conditions are met. The daemon stamps closed_count
    /// when every (watched_name, watched_word, threshold) condition is satisfied.
    ///
    /// Returns WaitResult with completed_at = clock.open_count at the time
    /// all conditions were met.
    pub fn wait(&self) -> Result<WaitResult, SdkError> {
        let words = self.handle.words();
        let closed_word = &words.closed_count;

        loop {
            let closed = closed_word.load(Ordering::Acquire);
            let open = words.open_count.load(Ordering::Acquire);

            // The daemon stamps closed_count when all conditions are met.
            // value <= 0 means the barrier fired.
            let value = (open as i64).wrapping_sub(closed as i64);
            if value <= 0 && closed > 0 {
                let state = if closed == open {
                    WaitState::Normal
                } else {
                    WaitState::Overrun
                };
                return Ok(WaitResult {
                    completed_at: closed,
                    state,
                });
            }

            let exp = words.expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || closed == SENTINEL || open == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }

            let lo32 = futex_word(closed);
            futex_wait(closed_word, lo32, DEFAULT_TIMEOUT_NANOS);

            let exp = words.expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    /// Read the daemon's response (closed_count): the clock value when all
    /// conditions were met.
    pub fn completed_at(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
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

impl Drop for WaitBarrier {
    fn drop(&mut self) {
        self.touch_thread.take();
    }
}
