//! Background touch thread for interlock keepalive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use abacus_core::clock::ms_to_nanos;
use abacus_core::interlock::{interlock_arm, InterlockHandle};

/// A background thread that periodically refreshes an interlock's expiration.
/// Dropping the TouchThread stops the background thread.
///
/// If the interlock is reaped (interlock_arm returns Err), the thread sets
/// the reaped flag and exits. Check is_reaped() to detect this condition.
pub struct TouchThread {
    stop: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TouchThread {
    /// Spawn a touch thread that refreshes `interlock`'s expiration every
    /// `interval_ms` milliseconds. The TTL set on each touch is `2 * interval_ms`
    /// to provide margin.
    pub fn spawn(interlock: InterlockHandle, interval_ms: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let reaped = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let reaped_flag = reaped.clone();
        let ttl_nanos = ms_to_nanos(interval_ms.saturating_mul(2));

        let handle = thread::spawn(move || {
            let sleep_dur = Duration::from_millis(interval_ms);
            while !stop_flag.load(Ordering::Acquire) {
                // Extend the expiration. If the interlock was reaped, signal
                // the owner via the reaped flag and exit.
                if interlock_arm(&interlock, ttl_nanos).is_err() {
                    reaped_flag.store(true, Ordering::Release);
                    break;
                }
                thread::sleep(sleep_dur);
            }
        });

        Self {
            stop,
            reaped,
            handle: Some(handle),
        }
    }

    /// Signal the touch thread to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Returns true if the interlock was reaped while the thread was running.
    pub fn is_reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }
}

impl Drop for TouchThread {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
