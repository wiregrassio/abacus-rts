//! Integration test harness for Abacus RTS.
//!
//! Tests run against a live daemon started in a background thread. Each test
//! gets its own socket path so tests can run in parallel.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a unique socket path for a test.
pub fn test_socket_path(label: &str) -> PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("abacus-test-{label}-{pid}-{id}.sock"))
}

/// Start the daemon in a background thread. Returns the socket path.
/// The daemon runs until the thread is abandoned (test process exits).
pub fn start_daemon(label: &str) -> PathBuf {
    let path = test_socket_path(label);
    let daemon_path = path.clone();
    std::thread::Builder::new()
        .name(format!("daemon-{label}"))
        .spawn(move || {
            if let Err(e) = abacus_daemon::daemon::daemon_run(&daemon_path) {
                eprintln!("test daemon failed: {e}");
            }
        })
        .expect("failed to spawn daemon thread");

    // Wait for the socket to appear (daemon is listening).
    wait_for_socket(&path, Duration::from_secs(2));
    path
}

/// Poll until the socket file exists, or panic after the timeout.
fn wait_for_socket(path: &Path, timeout: Duration) {
    let start = std::time::Instant::now();
    while !path.exists() {
        if start.elapsed() > timeout {
            panic!(
                "daemon did not create socket at {} within {:?}",
                path.display(),
                timeout
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Extra settle time for the daemon to start its loop.
    std::thread::sleep(Duration::from_millis(50));
}

/// Clean up a socket file. Best-effort.
pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}
