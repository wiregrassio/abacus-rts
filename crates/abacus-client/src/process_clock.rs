//! ProcessClock: an interlock used as a liveness and uptime beacon.
//!
//! The SDK touch thread updates both expiration (keepalive) and open_count
//! (current clock time) on each tick. Other processes attach read-only and
//! read value for uptime. If the process dies, touches stop, TTL lapses,
//! daemon reaps, watchers get InterlockReaped.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use abacus_core::clock::ms_to_nanos;
use abacus_core::interlock::{interlock_arm, interlock_free, InterlockHandle};

use crate::types::DEFAULT_TOUCH_INTERVAL_MS;

/// A ProcessClock wraps a bare interlock with a custom touch thread that
/// updates both expiration_ns (keepalive) and open_count (current monotonic
/// time from the system clock) on each tick.
///
/// Field mapping:
///   closed_count = start time (clock.open_count at creation, never changes)
///   open_count   = last seen time (updated to clock.open_count on each touch)
///   value        = uptime in milliseconds
pub struct ProcessClock {
    handle: InterlockHandle,
    #[allow(dead_code)]
    clock: InterlockHandle,
    touch_thread: Option<ProcessClockTouchThread>,
}

impl ProcessClock {
    /// Create a new ProcessClock from a bare interlock handle and the system
    /// clock handle. Sets closed_count to the current clock time (start time)
    /// and starts the custom touch thread.
    pub(crate) fn new(handle: InterlockHandle, clock: InterlockHandle) -> Self {
        // Set closed_count = clock.open_count (start time, set once).
        let start_time = clock.words().open_count.load(Ordering::Acquire);
        handle.words().closed_count.store(start_time, Ordering::Release);
        // Set open_count = same start time initially.
        handle.words().open_count.store(start_time, Ordering::Release);

        let touch_thread = Some(ProcessClockTouchThread::spawn(
            handle.clone(),
            clock.clone(),
            DEFAULT_TOUCH_INTERVAL_MS,
        ));

        Self {
            handle,
            clock,
            touch_thread,
        }
    }

    /// Uptime in milliseconds: open_count - closed_count.
    pub fn uptime_ms(&self) -> i64 {
        let words = self.handle.words();
        let open = words.open_count.load(Ordering::Acquire);
        let closed = words.closed_count.load(Ordering::Acquire);
        (open as i64).wrapping_sub(closed as i64)
    }

    /// Last seen time in monotonic milliseconds (reads open_count).
    pub fn last_seen_ms(&self) -> u64 {
        self.handle.words().open_count.load(Ordering::Acquire)
    }

    /// Start time in monotonic milliseconds (reads closed_count).
    pub fn start_time_ms(&self) -> u64 {
        self.handle.words().closed_count.load(Ordering::Acquire)
    }

    /// Returns true if the touch thread detected the interlock was reaped.
    pub fn is_reaped(&self) -> bool {
        self.touch_thread
            .as_ref()
            .map_or(true, |tt| tt.is_reaped())
    }

    pub fn free(&mut self) {
        self.touch_thread.take();
        interlock_free(&self.handle);
    }
}

impl Drop for ProcessClock {
    fn drop(&mut self) {
        self.touch_thread.take();
    }
}

/// Custom touch thread for ProcessClock. In addition to refreshing
/// expiration_ns, it sets open_count = clock.open_count on each tick.
struct ProcessClockTouchThread {
    stop: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProcessClockTouchThread {
    fn spawn(
        interlock: InterlockHandle,
        clock: InterlockHandle,
        interval_ms: u64,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let reaped = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let reaped_flag = reaped.clone();
        let ttl_nanos = ms_to_nanos(interval_ms.saturating_mul(2));

        let handle = thread::spawn(move || {
            let sleep_dur = Duration::from_millis(interval_ms);
            while !stop_flag.load(Ordering::Acquire) {
                // Refresh expiration (keepalive).
                if interlock_arm(&interlock, ttl_nanos).is_err() {
                    reaped_flag.store(true, Ordering::Release);
                    break;
                }
                // Update open_count to current clock time.
                let clock_now = clock.words().open_count.load(Ordering::Acquire);
                interlock.words().open_count.store(clock_now, Ordering::Release);

                thread::sleep(sleep_dur);
            }
        });

        Self {
            stop,
            reaped,
            handle: Some(handle),
        }
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn is_reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }
}

impl Drop for ProcessClockTouchThread {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
