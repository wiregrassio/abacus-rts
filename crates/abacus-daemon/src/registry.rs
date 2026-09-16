use std::collections::HashMap;
use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wake, expiration_alive, monotonic_now_nanos};
use abacus_core::error::{Condition, Result};
use abacus_core::interlock::{
    interlock_arm, interlock_create, interlock_dup_fd, interlock_read_expiration, interlock_reap,
    InterlockHandle,
};

// -- Tier --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Interlock,
    WaitCounter,
    WaitTimer,
    WaitCron,
    WaitBarrier,
}

/// Which word of the watched interlock to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedWord {
    OpenCount,
    ClosedCount,
}

// -- Registry --

pub struct InterlockEntry {
    pub id: u64,
    pub handle: InterlockHandle,
    pub tier: Tier,
    pub watched_name: Option<String>,
    pub watched_word: Option<WatchedWord>,
    /// WaitCron: grid interval in milliseconds.
    pub interval_ms: Option<u64>,
    /// WaitBarrier: list of (watched_name, watched_word, threshold) conditions.
    pub barrier_conditions: Option<Vec<(String, WatchedWord, u64)>>,
}

pub struct Registry {
    next_id: u64,
    by_name: HashMap<String, InterlockEntry>,
}

impl Registry {
    pub fn new() -> Result<Self> {
        let clock = interlock_create()?;
        // Extend clock expiration to 1s (creation gives 100ms).
        interlock_arm(&clock, 1_000_000_000)
            .expect("fresh clock cannot be reaped");

        // Initialize clock with monotonic ms timestamps.
        // closed_count = daemon start monotonic ms (set once, never changes).
        // open_count = current monotonic ms (updated each cycle by the daemon loop).
        let start_ms = monotonic_now_nanos() / 1_000_000;
        clock.words().closed_count.store(start_ms, Ordering::Release);
        clock.words().open_count.store(start_ms, Ordering::Release);

        let mut by_name = HashMap::new();
        by_name.insert(
            "clock".to_string(),
            InterlockEntry {
                id: 0,
                handle: clock,
                tier: Tier::Interlock,
                watched_name: None,
                watched_word: None,
                interval_ms: None,
                barrier_conditions: None,
            },
        );
        Ok(Self {
            next_id: 1,
            by_name,
        })
    }

    pub fn clock(&self) -> &InterlockHandle {
        &self
            .by_name
            .get("clock")
            .expect("clock interlock missing")
            .handle
    }

    pub fn create(
        &mut self,
        name: String,
        tier: Tier,
        watched_name: Option<String>,
        watched_word: Option<u8>,
        interval_ns: Option<u64>,
        barrier_conditions: Option<Vec<(String, u8, u64)>>,
    ) -> Result<(u64, InterlockHandle)> {
        // "clock" is a reserved system interlock; reject client creation attempts.
        if name == "clock" {
            return Err(Condition::InvalidRequest {
                message: "\"clock\" is a reserved name".to_string(),
            });
        }

        let mut resolved_name = None;
        let mut resolved_word = None;
        let mut resolved_interval_ms = None;
        let mut resolved_barrier: Option<Vec<(String, WatchedWord, u64)>> = None;

        match tier {
            Tier::WaitCounter => {
                let wn = watched_name.ok_or_else(|| Condition::InvalidRequest {
                    message: "WaitCounter requires watched_name".to_string(),
                })?;
                if !self.by_name.contains_key(&wn) {
                    return Err(Condition::InterlockNotFound { name: wn });
                }
                let ww = watched_word.ok_or_else(|| Condition::InvalidRequest {
                    message: "WaitCounter requires watched_word".to_string(),
                })?;
                let word = match ww {
                    0 => WatchedWord::OpenCount,
                    1 => WatchedWord::ClosedCount,
                    _ => {
                        return Err(Condition::InvalidRequest {
                            message: format!("invalid watched_word: {ww}"),
                        });
                    }
                };
                resolved_name = Some(wn);
                resolved_word = Some(word);
            }
            Tier::WaitTimer => {
                resolved_name = Some("clock".to_string());
                resolved_word = Some(WatchedWord::OpenCount);
            }
            Tier::WaitCron => {
                let ival_ns = interval_ns.ok_or_else(|| Condition::InvalidRequest {
                    message: "WaitCron requires interval_ns".to_string(),
                })?;
                if ival_ns == 0 {
                    return Err(Condition::InvalidRequest {
                        message: "WaitCron interval_ns must be > 0".to_string(),
                    });
                }
                let ival_ms = ival_ns / 1_000_000;
                if ival_ms == 0 {
                    return Err(Condition::InvalidRequest {
                        message: format!(
                            "WaitCron interval_ns ({ival_ns}) is less than 1ms"
                        ),
                    });
                }
                resolved_interval_ms = Some(ival_ms);
                resolved_name = Some("clock".to_string());
                resolved_word = Some(WatchedWord::OpenCount);
            }
            Tier::WaitBarrier => {
                let conds = barrier_conditions.ok_or_else(|| Condition::InvalidRequest {
                    message: "WaitBarrier requires conditions".to_string(),
                })?;
                let mut parsed: Vec<(String, WatchedWord, u64)> = Vec::with_capacity(conds.len());
                for (wn, ww_byte, threshold) in conds {
                    if !self.by_name.contains_key(&wn) {
                        return Err(Condition::InterlockNotFound { name: wn });
                    }
                    let word = match ww_byte {
                        0 => WatchedWord::OpenCount,
                        1 => WatchedWord::ClosedCount,
                        _ => {
                            return Err(Condition::InvalidRequest {
                                message: format!("invalid watched_word: {ww_byte}"),
                            });
                        }
                    };
                    parsed.push((wn, word, threshold));
                }
                resolved_barrier = Some(parsed);
            }
            Tier::Interlock => {}
        }

        let handle = interlock_create()?;
        let id = self.next_id;
        self.next_id += 1;

        // Tier-specific initialization per CONTRACTS.md.
        let clock_now_ms = self.clock().words().open_count.load(Ordering::Acquire);
        match tier {
            Tier::WaitTimer => {
                // open_count = clock.open_count (now), closed_count = clock.open_count (now).
                handle.words().open_count.store(clock_now_ms, Ordering::Release);
                handle.words().closed_count.store(clock_now_ms, Ordering::Release);
            }
            Tier::WaitCron => {
                // open_count = next grid line, closed_count = clock.open_count (now).
                let interval_ms = resolved_interval_ms.unwrap();
                let next_line = ((clock_now_ms / interval_ms) + 1) * interval_ms;
                handle.words().open_count.store(next_line, Ordering::Release);
                handle.words().closed_count.store(clock_now_ms, Ordering::Release);
            }
            Tier::WaitBarrier => {
                // open_count = 1 to make the interlock Open so the daemon evaluates.
                // closed_count = 0.
                handle.words().open_count.store(1, Ordering::Release);
            }
            _ => {
                // Interlock, WaitCounter: open_count = 0, closed_count = 0.
                // Already zero from interlock_create.
            }
        }

        // Wildebeest Mode: if the name exists, reap the old interlock and replace.
        if let Some(old) = self.by_name.remove(&name) {
            interlock_reap(&old.handle);
        }

        self.by_name.insert(
            name,
            InterlockEntry {
                id,
                handle: handle.clone(),
                tier,
                watched_name: resolved_name,
                watched_word: resolved_word,
                interval_ms: resolved_interval_ms,
                barrier_conditions: resolved_barrier,
            },
        );

        Ok((id, handle))
    }

    pub fn attach(&self, name: &str) -> Result<(u64, InterlockHandle)> {
        match self.by_name.get(name) {
            Some(entry) => Ok((entry.id, entry.handle.clone())),
            None => Err(Condition::InterlockNotFound {
                name: name.to_string(),
            }),
        }
    }

    /// Evaluate all interlocks in a single pass. For each (excluding "clock"):
    ///   1. TTL check: if expired, mark for removal and reap.
    ///   2. Compute value = open_count - closed_count.
    ///   3. If Open and wait type, evaluate the wait contract and wake if met.
    /// After iteration, remove expired entries from the map.
    pub fn evaluate_all(&mut self, clock_now_ms: u64) {
        let now_ns = monotonic_now_nanos();

        let mut to_remove: Vec<String> = Vec::new();

        for (name, entry) in self.by_name.iter() {
            if name == "clock" {
                continue;
            }

            // TTL check.
            let exp = interlock_read_expiration(&entry.handle);
            if !expiration_alive(exp, now_ns) {
                interlock_reap(&entry.handle);
                to_remove.push(name.clone());
                continue;
            }

            // Compute value.
            let words = entry.handle.words();
            let open_count = words.open_count.load(Ordering::Acquire);
            let closed_count = words.closed_count.load(Ordering::Acquire);
            let value = (open_count as i64).wrapping_sub(closed_count as i64);

            // Only evaluate wait contract for Open interlocks with a wait type.
            if value <= 0 {
                continue;
            }

            match entry.tier {
                Tier::WaitTimer => {
                    if clock_now_ms >= open_count {
                        words.closed_count.store(clock_now_ms, Ordering::Release);
                        futex_wake(&words.closed_count);
                    }
                }
                Tier::WaitCounter => {
                    if let (Some(wn), Some(ww)) = (entry.watched_name.as_ref(), entry.watched_word) {
                        if let Some(watched_entry) = self.by_name.get(wn) {
                            let watched_val = match ww {
                                WatchedWord::OpenCount => {
                                    watched_entry.handle.words().open_count.load(Ordering::Acquire)
                                }
                                WatchedWord::ClosedCount => {
                                    watched_entry.handle.words().closed_count.load(Ordering::Acquire)
                                }
                            };
                            if watched_val >= open_count {
                                words.closed_count.store(watched_val, Ordering::Release);
                                futex_wake(&words.closed_count);
                            }
                        }
                    }
                }
                Tier::WaitCron => {
                    // Fire if clock has reached or passed the grid line.
                    if clock_now_ms >= open_count {
                        words.closed_count.store(clock_now_ms, Ordering::Release);
                        futex_wake(&words.closed_count);
                        // Auto-re-arm to next grid line.
                        if let Some(interval_ms) = entry.interval_ms {
                            let next_line = ((clock_now_ms / interval_ms) + 1) * interval_ms;
                            words.open_count.store(next_line, Ordering::Release);
                        }
                    }
                }
                Tier::WaitBarrier => {
                    // Check all conditions; fire if all met.
                    if let Some(ref conditions) = entry.barrier_conditions {
                        let all_met = conditions.iter().all(|(wn, ww, threshold)| {
                            if let Some(watched_entry) = self.by_name.get(wn) {
                                let watched_val = match ww {
                                    WatchedWord::OpenCount => {
                                        watched_entry.handle.words().open_count.load(Ordering::Acquire)
                                    }
                                    WatchedWord::ClosedCount => {
                                        watched_entry.handle.words().closed_count.load(Ordering::Acquire)
                                    }
                                };
                                watched_val >= *threshold
                            } else {
                                false
                            }
                        });
                        if all_met {
                            words.closed_count.store(clock_now_ms, Ordering::Release);
                            futex_wake(&words.closed_count);
                        }
                    }
                }
                Tier::Interlock => {}
            }
        }

        // Remove expired entries (cannot remove during HashMap iteration).
        for name in to_remove {
            self.by_name.remove(&name);
        }
    }

    pub fn refresh_clock_expiration(&self) {
        // Clock is never reaped; ignore the result.
        let _ = interlock_arm(self.clock(), 1_000_000_000);
    }

    pub fn dup_clock_fd(&self) -> Result<std::os::fd::OwnedFd> {
        interlock_dup_fd(self.clock())
    }
}
