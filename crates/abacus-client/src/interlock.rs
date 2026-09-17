//! Interlock and AttachedInterlock handles.
//!
//! The base primitive. Owner writes both counters freely. Daemon only reaps on
//! expired TTL.

use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wait, futex_wake, futex_word, monotonic_now_nanos, ms_to_nanos};
use abacus_core::interlock::{interlock_arm, interlock_free, InterlockHandle, SENTINEL};

use crate::types::{interlock_state, InterlockState, DEFAULT_TIMEOUT_NANOS, DEFAULT_TOUCH_INTERVAL_MS};

use crate::client::SdkError;
use crate::touch::TouchThread;

/// An interlock created by this client. Full read/write access to both counters
/// and expiration.
pub struct Interlock {
    handle: InterlockHandle,
    touch_thread: Option<TouchThread>,
}

impl Interlock {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        let touch_thread = Some(TouchThread::spawn(handle.clone(), DEFAULT_TOUCH_INTERVAL_MS));
        Self {
            handle,
            touch_thread,
        }
    }

    pub fn open(&self, h: u64) {
        let word = &self.handle.words().open_count;
        word.fetch_add(h, Ordering::Release);
        futex_wake(word);
    }

    pub fn close(&self, h: u64) {
        let word = &self.handle.words().closed_count;
        word.fetch_add(h, Ordering::Release);
        futex_wake(word);
    }

    /// Extend expiration to max(current, now + ms). CAS-max via interlock_arm.
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

    pub fn wait_open(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().open_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    pub fn wait_close(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().closed_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    /// Signed difference: open_count - closed_count.
    ///
    /// Wrapping i64 subtraction. Counters above i64::MAX (2^63) are outside the
    /// spec's representable range. No defined tier reaches this.
    pub fn value(&self) -> i64 {
        let (open, closed) = self.peek();
        (open as i64).wrapping_sub(closed as i64)
    }

    /// Derive the lifecycle state (Open, Closed, Overrun, Expired) from the
    /// current counter values and expiration. Uses CLOCK_MONOTONIC internally.
    pub fn state(&self) -> InterlockState {
        let words = self.handle.words();
        let open = words.open_count.load(Ordering::Acquire);
        let closed = words.closed_count.load(Ordering::Acquire);
        let expiration_ns = words.expiration_ns.load(Ordering::Acquire);
        let clock_ns = monotonic_now_nanos();
        interlock_state(open, closed, expiration_ns, clock_ns)
    }

    /// Start (or replace) the background touch thread with the given interval.
    pub fn start_touch_thread(&mut self, interval_ms: u64) -> &TouchThread {
        // Stop any existing thread.
        self.touch_thread.take();
        let tt = TouchThread::spawn(self.handle.clone(), interval_ms);
        self.touch_thread = Some(tt);
        self.touch_thread.as_ref().unwrap()
    }

    /// Stop the background touch thread. The interlock's heartbeat will lapse
    /// and the daemon will reap it unless manually touched.
    pub fn stop_touch_thread(&mut self) {
        self.touch_thread.take();
    }

    /// Terminate this interlock. Stops the touch thread and sets the terminal
    /// sentinel. The daemon cleans up on its next cycle.
    pub fn free(&mut self) {
        self.stop_touch_thread();
        interlock_free(&self.handle);
    }
}

impl Drop for Interlock {
    fn drop(&mut self) {
        // Touch thread is dropped automatically via Option<TouchThread>.
        self.touch_thread.take();
    }
}

/// An interlock attached by name. Attacher has r/w on open_count and
/// closed_count, r/o on expiration (no touch, no touch thread).
pub struct AttachedInterlock {
    handle: InterlockHandle,
}

impl AttachedInterlock {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        Self { handle }
    }

    pub fn open(&self, h: u64) {
        let word = &self.handle.words().open_count;
        word.fetch_add(h, Ordering::Release);
        futex_wake(word);
    }

    pub fn close(&self, h: u64) {
        let word = &self.handle.words().closed_count;
        word.fetch_add(h, Ordering::Release);
        futex_wake(word);
    }

    /// Read both counters: (open_count, closed_count).
    pub fn peek(&self) -> (u64, u64) {
        let words = self.handle.words();
        let open = words.open_count.load(Ordering::Acquire);
        let closed = words.closed_count.load(Ordering::Acquire);
        (open, closed)
    }

    pub fn wait_open(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().open_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    pub fn wait_close(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().closed_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    pub fn value(&self) -> i64 {
        let (open, closed) = self.peek();
        (open as i64).wrapping_sub(closed as i64)
    }

    pub fn state(&self) -> InterlockState {
        let words = self.handle.words();
        let open = words.open_count.load(Ordering::Acquire);
        let closed = words.closed_count.load(Ordering::Acquire);
        let expiration_ns = words.expiration_ns.load(Ordering::Acquire);
        let clock_ns = monotonic_now_nanos();
        interlock_state(open, closed, expiration_ns, clock_ns)
    }

    /// Terminate this interlock by writing SENTINEL to open_count. Attachers
    /// have r/o on expiration, so they terminate via a counter.
    pub fn free(&self) {
        let word = &self.handle.words().open_count;
        word.store(SENTINEL, Ordering::Release);
        futex_wake(word);
        futex_wake(&self.handle.words().closed_count);
    }
}

/// A read-only view of a WaitCounter, obtained via attach. No open, close,
/// touch, or wait methods. The creator owns all mutations; the attacher
/// observes.
pub struct AttachedWaitCounter {
    handle: InterlockHandle,
}

impl AttachedWaitCounter {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        Self { handle }
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

    /// Read the daemon's response (closed_count).
    pub fn completed_at(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
    }
}

/// A read-only view of the system clock interlock. The daemon owns the clock;
/// clients observe via this handle.
///
/// Field mapping:
///   open_count   = current monotonic time in ms (updated every daemon cycle)
///   closed_count = daemon start monotonic ms (set once at startup)
///   value        = uptime in ms
pub struct ClockHandle {
    handle: InterlockHandle,
}

impl ClockHandle {
    pub(crate) fn new(handle: InterlockHandle) -> Self {
        Self { handle }
    }

    /// Read both counters: (now_ms, start_time_ms).
    pub fn peek(&self) -> (u64, u64) {
        let words = self.handle.words();
        let now = words.open_count.load(Ordering::Acquire);
        let start = words.closed_count.load(Ordering::Acquire);
        (now, start)
    }

    /// Uptime in milliseconds: open_count - closed_count.
    ///
    /// Wrapping i64 subtraction. Counters above i64::MAX (2^63) are outside the
    /// spec's representable range. No defined tier reaches this.
    pub fn value(&self) -> i64 {
        let (now, start) = self.peek();
        (now as i64).wrapping_sub(start as i64)
    }

    /// Current monotonic time in milliseconds (reads open_count).
    pub fn now_ms(&self) -> u64 {
        self.handle.words().open_count.load(Ordering::Acquire)
    }

    /// Daemon start time in monotonic milliseconds (reads closed_count).
    pub fn start_time_ms(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
    }

    /// Uptime in milliseconds.
    pub fn uptime_ms(&self) -> i64 {
        self.value()
    }

    pub fn wait_open(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().open_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    pub fn wait_close(&self, target: u64) -> Result<u64, SdkError> {
        let word = &self.handle.words().closed_count;
        loop {
            let current = word.load(Ordering::Acquire);
            if current == SENTINEL {
                return Err(SdkError::InterlockReaped);
            }
            if current >= target {
                return Ok(current);
            }
            let lo32 = futex_word(current);
            futex_wait(word, lo32, DEFAULT_TIMEOUT_NANOS);
            let exp = self.handle.words().expiration_ns.load(Ordering::Acquire);
            if exp == SENTINEL || exp < monotonic_now_nanos() {
                return Err(SdkError::InterlockReaped);
            }
        }
    }

    /// Access the underlying InterlockHandle. Crate-internal: used by client.rs
    /// to pass the clock handle to WaitTimer and ProcessClock construction.
    pub(crate) fn handle(&self) -> &InterlockHandle {
        &self.handle
    }
}
