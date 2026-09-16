//! SDK result types for wait operations.

/// Internal poll cadence for wait loops (100ms). Each futex_wait iteration uses this
/// as its timeout, after which the loop re-checks state (reaped, target reached) before
/// re-sleeping. Not a user-visible timeout.
pub const DEFAULT_TIMEOUT_NANOS: u64 = 100_000_000;

/// Default touch interval in milliseconds. 40 ms gives 2.5 ticks per 100 ms
/// creation TTL, ensuring at least two heartbeats land before expiration.
pub const DEFAULT_TOUCH_INTERVAL_MS: u64 = 40;

/// Outcome of a wait operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitState {
    /// closed_count == open_count: woken exactly at target.
    Normal,
    /// closed_count > open_count: daemon overshot the target.
    Overrun,
    /// Futex timed out; daemon never delivered.
    Timeout,
}

/// Result returned by wait_until (WaitCounter) and wait_ms (WaitTimer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitResult {
    /// The closed_count value when woken (the daemon's response).
    pub completed_at: u64,
    /// Whether the wake was normal, late, or timed out.
    pub state: WaitState,
}

/// Interlock lifecycle state, derived from value (open - closed) and TTL
/// (expiration_ns - clock_ns).
///
/// Per CONTRACTS.md field-semantics:
///   Closed:  value == 0, ttl > 0
///   Open:    value > 0, ttl > 0
///   Overrun: value < 0
///   Expired: ttl <= 0
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterlockState {
    /// value == 0, ttl > 0: all opened work has been closed.
    Closed,
    /// value > 0, ttl > 0: work is in progress.
    Open,
    /// value < 0: closed_count exceeded open_count.
    Overrun,
    /// ttl <= 0: heartbeat lapsed, daemon will reap.
    Expired,
}

/// Compute the interlock lifecycle state from raw field values.
///
/// `open` and `closed` are the counter values. `expiration_ns` is the
/// interlock's expiration in CLOCK_MONOTONIC nanoseconds. `clock_ns` is the
/// current CLOCK_MONOTONIC time in nanoseconds.
pub fn interlock_state(open: u64, closed: u64, expiration_ns: u64, clock_ns: u64) -> InterlockState {
    // Expired takes priority: if TTL has lapsed, the interlock is dead
    // regardless of counter state.
    if expiration_ns == 0 || clock_ns >= expiration_ns {
        return InterlockState::Expired;
    }
    let value = (open as i64).wrapping_sub(closed as i64);
    if value < 0 {
        InterlockState::Overrun
    } else if value > 0 {
        InterlockState::Open
    } else {
        InterlockState::Closed
    }
}

/// Which word of a watched interlock to compare against the target.
/// Maps to the wire protocol discriminant: 0 = open_count, 1 = closed_count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedWord {
    /// Watch open_count (word 0).
    OpenCount = 0,
    /// Watch closed_count (word 1).
    ClosedCount = 1,
}

impl WatchedWord {
    /// Wire protocol discriminant.
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// RTSTimeout: the daemon did not deliver within the timeout. For WaitTimer,
/// this is fatal (the SDK aborts the process). Corresponds to
/// WaitState::Timeout on a WaitTimer.
pub const RTS_TIMEOUT: WaitState = WaitState::Timeout;

/// RTSOverrun: the daemon delivered late (completed_at > wait_until).
/// Corresponds to WaitState::Overrun.
pub const RTS_OVERRUN: WaitState = WaitState::Overrun;

/// Determine the wake state from the open/closed counter pair.
pub fn check_wake_state(open: u64, closed: u64) -> WaitState {
    if closed == open {
        WaitState::Normal
    } else if closed > open {
        WaitState::Overrun
    } else {
        // open > closed: daemon never delivered
        WaitState::Timeout
    }
}
