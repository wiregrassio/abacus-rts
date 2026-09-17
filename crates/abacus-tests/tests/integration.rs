//! Integration tests for Abacus RTS.
//!
//! Each test starts a daemon in a background thread and exercises the SDK
//! against it. Tests prove the client contract, not internal daemon logic.

use std::time::{Duration, Instant};

use abacus_client::{
    AbacusClient, WatchedWord, WaitState,
};
use abacus_tests::{start_daemon, cleanup};

// ---------------------------------------------------------------------------
// 1. Timer fires
// ---------------------------------------------------------------------------

#[test]
fn timer_fires_within_tolerance() {
    let path = start_daemon("timer-fires");
    let mut client = AbacusClient::connect(&path).unwrap();

    let timer = client.create_wait_timer("t1").unwrap();
    let start = Instant::now();
    let result = timer.wait_ms(50).unwrap();
    let elapsed = start.elapsed();

    assert!(
        matches!(result.state, WaitState::Normal | WaitState::Overrun),
        "expected Normal or Overrun, got {:?}",
        result.state
    );
    assert!(
        elapsed < Duration::from_millis(100),
        "timer took too long: {:?}",
        elapsed
    );
    assert!(
        elapsed >= Duration::from_millis(45),
        "timer returned too early: {:?}",
        elapsed
    );

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 2. Counter crosses
// ---------------------------------------------------------------------------

#[test]
fn wait_counter_wakes_on_interlock_advance() {
    let path = start_daemon("counter-crosses");
    let mut client = AbacusClient::connect(&path).unwrap();

    // Create a source interlock and a WaitCounter watching its closed_count.
    let source = client.create_interlock("source").unwrap();
    let counter = client.create_wait_counter(
        "watcher",
        "source",
        WatchedWord::ClosedCount,
    ).unwrap();

    // Advance the source in a background thread after a short delay.
    // Use the public API: source.close(10) advances closed_count by 10.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        source.close(10);
    });

    // Wait for the counter to cross target 5 (500ms timeout).
    let result = counter.wait_until(5, 500).unwrap();

    assert!(
        matches!(result.state, WaitState::Normal | WaitState::Overrun),
        "expected Normal or Overrun, got {:?}",
        result.state
    );
    assert!(
        result.completed_at >= 5,
        "completed_at should be >= 5, got {}",
        result.completed_at
    );

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 3. Heartbeat reap
// ---------------------------------------------------------------------------

#[test]
fn interlock_is_reaped_when_not_touched() {
    let path = start_daemon("heartbeat-reap");
    let mut client = AbacusClient::connect(&path).unwrap();

    // Create an interlock and stop its auto-started touch thread.
    // Without touches, the creation TTL (100ms) will expire.
    let mut il = client.create_interlock("ephemeral").unwrap();
    il.stop_touch_thread();

    // Wait for the daemon to reap it (100ms TTL + margin).
    std::thread::sleep(Duration::from_millis(150));

    // Touch should return InterlockReaped.
    let result = il.touch(100);
    assert!(result.is_err(), "expected InterlockReaped after TTL lapsed");

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 4. Wildebeest create-over-existing
// ---------------------------------------------------------------------------

#[test]
fn wildebeest_reaps_old_interlock() {
    let path = start_daemon("wildebeest");
    let mut client = AbacusClient::connect(&path).unwrap();

    // Create a named interlock and keep it alive.
    let mut il1 = client.create_interlock("contested").unwrap();
    let _touch1 = il1.start_touch_thread(40);
    il1.open(42);

    // Create the same name again (Wildebeest Mode).
    let il2 = client.create_interlock("contested").unwrap();

    // Give the daemon a cycle to process the reap.
    std::thread::sleep(Duration::from_millis(5));

    // The old handle should detect reap on next touch.
    let result = il1.touch(100);
    assert!(result.is_err(), "old interlock should be reaped after name collision");

    // New interlock should be fresh.
    let (open, closed) = il2.peek();
    assert_eq!(open, 0);
    assert_eq!(closed, 0);

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 5. Clock reads
// ---------------------------------------------------------------------------

#[test]
fn clock_advances() {
    let path = start_daemon("clock-reads");
    let client = AbacusClient::connect(&path).unwrap();

    let clock = client.clock();
    let t1 = clock.now_ms();
    std::thread::sleep(Duration::from_millis(50));
    let t2 = clock.now_ms();

    assert!(t2 > t1, "clock should advance: t1={t1}, t2={t2}");
    assert!(
        t2 - t1 >= 40,
        "clock should have advanced at least 40ms, got {}ms",
        t2 - t1
    );

    let uptime = clock.uptime_ms();
    assert!(uptime > 0, "uptime should be positive: {uptime}");

    let start_time = clock.start_time_ms();
    assert!(start_time > 0, "start time should be positive: {start_time}");

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 6. attach_interlock("clock") is rejected
// ---------------------------------------------------------------------------

#[test]
fn attach_clock_by_name_is_rejected() {
    let path = start_daemon("clock-reject");
    let mut client = AbacusClient::connect(&path).unwrap();

    let result = client.attach_interlock("clock");
    assert!(result.is_err(), "attaching to 'clock' by name should be rejected");

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 7. ProcessClock shows uptime
// ---------------------------------------------------------------------------

#[test]
fn process_clock_tracks_uptime() {
    let path = start_daemon("process-clock");
    let mut client = AbacusClient::connect(&path).unwrap();

    let pclock = client.create_process_clock("my-process").unwrap();

    // Give the touch thread a few ticks to update open_count.
    std::thread::sleep(Duration::from_millis(100));

    let uptime = pclock.uptime_ms();
    assert!(
        uptime >= 50,
        "process clock uptime should be at least 50ms, got {uptime}"
    );

    let last_seen = pclock.last_seen_ms();
    assert!(last_seen > 0, "last_seen should be positive");

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 8. Attach to a WaitCounter is read-only
// ---------------------------------------------------------------------------

#[test]
fn attached_wait_counter_is_read_only() {
    let path = start_daemon("attached-ro");
    let mut client = AbacusClient::connect(&path).unwrap();

    let _source = client.create_interlock("src").unwrap();
    let _counter = client.create_wait_counter(
        "watched",
        "src",
        WatchedWord::ClosedCount,
    ).unwrap();

    let attached = client.attach_wait_counter("watched").unwrap();
    let (open, closed) = attached.peek();
    let val = attached.value();

    // AttachedWaitCounter has peek() and value() but no open(), close(), or touch().
    // This test verifies the type compiles with read-only access.
    assert_eq!(open, 0);
    assert_eq!(closed, 0);
    assert_eq!(val, 0);

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 9. WaitTimer::wait_until (absolute time sugar)
// ---------------------------------------------------------------------------

#[test]
fn wait_until_absolute_time() {
    let path = start_daemon("wait-until");
    let mut client = AbacusClient::connect(&path).unwrap();

    let timer = client.create_wait_timer("t-abs").unwrap();
    let clock = client.clock();

    // Wait until 50ms from now.
    let target = clock.now_ms() + 50;
    let start = Instant::now();
    let result = timer.wait_until(target).unwrap();
    let elapsed = start.elapsed();

    assert!(
        matches!(result.state, WaitState::Normal | WaitState::Overrun),
        "expected Normal or Overrun, got {:?}",
        result.state
    );
    assert!(
        elapsed >= Duration::from_millis(40),
        "wait_until returned too early: {:?}",
        elapsed
    );

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 10. Multiple timers fire independently
// ---------------------------------------------------------------------------

#[test]
fn multiple_timers_independent() {
    let path = start_daemon("multi-timer");
    let mut client = AbacusClient::connect(&path).unwrap();

    let t1 = client.create_wait_timer("fast").unwrap();
    let t2 = client.create_wait_timer("slow").unwrap();

    let start = Instant::now();

    // Fast timer: 20ms.
    let r1 = t1.wait_ms(20).unwrap();
    let elapsed_fast = start.elapsed();

    // Slow timer: 80ms from original start.
    let r2 = t2.wait_ms(80).unwrap();
    let elapsed_slow = start.elapsed();

    assert!(matches!(r1.state, WaitState::Normal | WaitState::Overrun));
    assert!(matches!(r2.state, WaitState::Normal | WaitState::Overrun));

    assert!(elapsed_fast < Duration::from_millis(60));
    assert!(elapsed_slow >= Duration::from_millis(70));

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 11. Interlock open/close/value
// ---------------------------------------------------------------------------

#[test]
fn interlock_open_close_value() {
    let path = start_daemon("open-close");
    let mut client = AbacusClient::connect(&path).unwrap();

    let il = client.create_interlock("worker").unwrap();

    assert_eq!(il.value(), 0); // Closed
    il.open(3);
    assert_eq!(il.value(), 3); // Open (3 in flight)
    il.close(1);
    assert_eq!(il.value(), 2); // Still open (2 in flight)
    il.close(2);
    assert_eq!(il.value(), 0); // Closed again

    let (open, closed) = il.peek();
    assert_eq!(open, 3);
    assert_eq!(closed, 3);

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 12. free() terminates an interlock immediately
// ---------------------------------------------------------------------------

#[test]
fn free_terminates_interlock() {
    let path = start_daemon("free-terminate");
    let mut client = AbacusClient::connect(&path).unwrap();

    let mut il = client.create_interlock("doomed").unwrap();
    il.open(1);

    // free() should set the sentinel and stop the touch thread.
    il.free();

    // Touch should detect the sentinel immediately (no daemon cycle needed).
    let result = il.touch(100);
    assert!(result.is_err(), "touch after free() should return InterlockReaped");

    // Give the daemon a cycle to clean up the registry entry.
    std::thread::sleep(Duration::from_millis(5));

    // Creating the same name should succeed (Wildebeest: the old entry is gone).
    let il2 = client.create_interlock("doomed").unwrap();
    let (open, closed) = il2.peek();
    assert_eq!(open, 0);
    assert_eq!(closed, 0);

    cleanup(&path);
}

// ---------------------------------------------------------------------------
// 13. Attacher free() terminates via counter sentinel
// ---------------------------------------------------------------------------

#[test]
fn attacher_free_terminates_interlock() {
    let path = start_daemon("attacher-free");
    let mut client = AbacusClient::connect(&path).unwrap();

    let _owner = client.create_interlock("shared").unwrap();
    let attached = client.attach_interlock("shared").unwrap();

    // Attacher free() writes SENTINEL to open_count.
    attached.free();

    // Give the daemon a cycle to see the sentinel and reap.
    std::thread::sleep(Duration::from_millis(5));

    // Discriminating check: the daemon must have already reaped the entry
    // from the sentinel (not Wildebeest). Attach should fail because the
    // name is gone from the registry.
    let attach_result = client.attach_interlock("shared");
    assert!(attach_result.is_err(), "attach after free() should return InterlockNotFound");

    cleanup(&path);
}
