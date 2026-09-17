//! WaitRace: SDK-only composition. Watches N WaitCounters and returns when
//! the first one fires. No daemon changes; polls all counters at 1 ms
//! resolution (matching the daemon's delivery cadence).

use std::time::Duration;

use crate::types::{check_wake_state, WaitResult};
use crate::wait_counter::WaitCounter;

/// Watches N WaitCounters. Returns the index and result of the first to fire.
///
/// Usage: create WaitCounters, set their targets (via `wait_until` on each in
/// separate threads, or by setting open_count directly), then poll with
/// `WaitRace::wait`.
///
/// This is a 1 ms polling loop. The daemon delivers at 1 ms resolution, so
/// polling at 1 ms matches. Not ideal for latency-critical paths, but correct
/// and simple for the "first of N" use case.
pub struct WaitRace {
    counters: Vec<WaitCounter>,
}

impl WaitRace {
    /// Create a WaitRace over the given counters. Targets must already be set
    /// on each counter (open_count written, TTL armed) before calling `wait`.
    pub fn new(counters: Vec<WaitCounter>) -> Self {
        Self { counters }
    }

    /// Poll all counters until one fires. Returns `(index, WaitResult)` where
    /// `index` is the position in the original Vec.
    ///
    /// Snapshots each counter's closed_count at entry and waits for any
    /// counter's closed_count to advance past its snapshot. This correctly
    /// handles counters that start at (0,0) and auto-re-arming tiers
    /// where open > closed is the normal post-fire state.
    ///
    /// Sleeps 1 ms between poll rounds via `thread::sleep`. This matches the
    /// daemon's 1 ms cycle resolution.
    pub fn wait(&self) -> (usize, WaitResult) {
        // Snapshot initial closed_count for each counter.
        let initial: Vec<u64> = self.counters.iter()
            .map(|c| c.peek().1)
            .collect();

        loop {
            for (i, counter) in self.counters.iter().enumerate() {
                let (open, closed) = counter.peek();
                if closed > initial[i] {
                    // This counter's closed_count advanced: daemon delivered.
                    let state = check_wake_state(open, closed);
                    return (
                        i,
                        WaitResult {
                            completed_at: closed,
                            state,
                        },
                    );
                }
            }
            // None ready. Sleep 1 ms (daemon delivers at 1 ms resolution).
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Number of counters in this race.
    pub fn len(&self) -> usize {
        self.counters.len()
    }

    /// True if no counters are being raced.
    pub fn is_empty(&self) -> bool {
        self.counters.is_empty()
    }
}
