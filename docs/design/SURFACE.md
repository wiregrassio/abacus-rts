# Abacus API surface

The SDK public interface. What a consumer calls, what it returns, how types compose. The Rust
SDK is the native core; the Python SDK is a pyo3 binding over it, so the API exists once.

<connection>

## Connection

```
AbacusClient::connect(socket_path) -> AbacusClient
    Connects to the daemon over UDS. Auto-attaches to the "clock" interlock.

client.is_connected() -> bool
    Check if the daemon connection is still alive. Uses MSG_PEEK to detect peer
    closure. The touch thread can use this for early daemon-death detection.
```

</connection>

<interlock-api>

## Interlock

The base primitive. Owner writes both counters. Daemon only reaps.

```
client.create_interlock(name) -> Interlock
    Create a named interlock. Returns a mapped handle.
    Wildebeest: if the name exists, the old one is reaped.

client.attach_interlock(name) -> AttachedInterlock
    Attach to an existing interlock by name. Returns a mapped handle.
    Attacher: open_count r/w, closed_count r/w, expiration r/o.
    attached.free(&self) writes SENTINEL to open_count and wakes waiters on both
    open_count and closed_count. The attacher has no write access to expiration_ns,
    so free() signals termination through the counters instead of through
    interlock_free (see Termination, below).

interlock.open(h)                       open_count += h
interlock.close(h)                      closed_count += h
interlock.touch(ms)                     expiration = max(expiration, now + ms)
interlock.peek() -> (open, closed)      read both counters
interlock.wait_open(target) -> u64      futex_wait on open_count >= target
interlock.wait_close(target) -> u64     futex_wait on closed_count >= target
interlock.value() -> i64                open_count - closed_count
interlock.stop_touch_thread(&mut self)  stops the touch thread without terminating
                                         the interlock (see Background touch, below)
interlock.free(&mut self)               stops the touch thread, then frees the
                                         interlock (see Termination, below)
```

wait_open and wait_close check the loaded word against SENTINEL (u64::MAX, the
terminal marker) before comparing it to target: SENTINEL would otherwise satisfy
any `>= target` comparison and be returned as a valid value. A word equal to
SENTINEL returns InterlockReaped immediately. After each futex wake, the same
wait also re-checks expiration_ns: if expiration_ns == SENTINEL, or expiration_ns
has already passed, wait_open/wait_close return InterlockReaped rather than
looping again.

### Derived state (read-only)

```
interlock.state() -> InterlockState::Closed | InterlockState::Open | InterlockState::Expired | InterlockState::Overrun
```

### Errors

| Error | When | Response |
|-------|------|----------|
| InterlockReaped | TTL lapsed, name claimed by another create, or a word read as SENTINEL (daemon reap or client free()) | recreate and resume, or crash |

</interlock-api>

<wait-counter-api>

## WaitCounter

Watches another interlock's open_count or closed_count. The daemon stamps closed_count when
the watched value crosses the target. Strict superset of Interlock.

```
client.create_wait_counter(name, watched_name, watched_word) -> WaitCounter
    watched_name: name of the interlock to watch
    watched_word: Open or Closed (which counter to compare)

wait_counter.wait_until(target, timeout_ms) -> Result<WaitResult, SdkError>
    Sets open_count = target (CAS-max: preserves the largest).
    timeout_ms is the futex poll cadence in milliseconds. TTL is set to
    2 * timeout_ms. Use 100 for standard waits, higher for long-running watches.
    Blocks (futex_wait on closed_count) until daemon stamps closed_count.
    Reaped detection checks expiration_ns == SENTINEL, not == 0: if SENTINEL,
    returns InterlockReaped. A genuine futex timeout (the OS reports ETIMEDOUT,
    expiration_ns is still a real value) is different: it returns
    WaitResult { state: Timeout } instead of an error, or loops and re-checks.
    Returns WaitResult { completed_at, state }.

wait_counter.touch(ms)                  extend TTL
wait_counter.peek() -> (open, closed)   read both counters
wait_counter.free(&mut self)            stops the touch thread, then frees the
                                         interlock (see Termination, below)
```

### SDK aliases

| Stored word | SDK name | Meaning |
|-------------|----------|---------|
| open_count | wait_until | the target the client set |
| closed_count | completed_at | what the daemon delivered |

### WaitResult

```
WaitResult {
    completed_at: u64,      the watched value when woken
    state: WaitState,       Normal | Overrun | Timeout
}

enum WaitState {
    Normal,     completed_at == wait_until
    Overrun,    completed_at > wait_until (daemon overshot)
    Timeout,    futex timed out, daemon never delivered
}
```

For WaitCounter, Timeout handling is the owner's decision. The SDK surfaces it; the owner
decides whether to retry, crash, or ignore. SENTINEL is a separate, harder condition: it
means the interlock itself was reaped or freed, not merely that the daemon has not yet
delivered, and it always raises InterlockReaped instead of returning a WaitResult.

</wait-counter-api>

<wait-timer-api>

## WaitTimer

A WaitCounter that watches clock.closed_count. SDK sugar for time-based waits. Strict
superset of WaitCounter.

```
client.create_wait_timer(name) -> WaitTimer
    Auto-watches clock.closed_count. No watched_name or watched_word needed.

wait_timer.wait_ms(ms) -> WaitResult
    Sets open_count = clock.open_count + ms (plain addition: clock_now + ms, not
    saturating_add). clock.open_count is the clock's advancing "current time"
    counter (see Clock, below).
    Raises expiration to max(current, now + 2 * ms).
    Blocks until daemon stamps closed_count.
    Reaped detection checks expiration_ns == SENTINEL, not == 0: if SENTINEL,
    returns InterlockReaped immediately, before the fatal-timeout check below.
    Returns WaitResult { completed_at, state }.

    If state == Timeout: RTSTimeout. Fatal. SDK crashes the process.
    If state == Overrun: RTSOverrun. SDK surfaces it; handling is caller-decided.

wait_timer.touch(ms)                    extend TTL
wait_timer.free(&mut self)              stops the touch thread, then frees the
                                         interlock (see Termination, below)
```

### Fatal timeout

The 2x TTL margin ensures the interlock cannot be reaped while legitimately waiting. If the
futex times out at 2*W and the daemon has not stamped closed_count, the daemon is dead. The
SDK raises RTSTimeout and crashes the process. Higher-level code never sees it: the blocking
wait is the liveness check.

The SENTINEL check runs first: if expiration_ns == SENTINEL (the interlock was reaped or
freed), wait_ms returns InterlockReaped rather than aborting. The abort path fires only when
the interlock is still nominally live (expiration_ns is a real, still-future value) but the
daemon simply failed to deliver within the deadline.

</wait-timer-api>

<clock-api>

## Clock

The daemon-owned system clock interlock. Attached by name "clock". Read-only. No free():
the system clock is daemon-owned and never terminated by clients.

```
client.clock() -> &ClockHandle
    The system clock, attached once on connect. Read-only.

clock.peek() -> (now_ms, start_time_ms)     read both counters
clock.now_ms() -> u64                        current monotonic time in ms (open_count)
clock.start_time_ms() -> u64                 daemon start monotonic ms (closed_count)
clock.uptime_ms() -> i64                     open_count - closed_count
clock.wait_open(target) -> Result<u64>       wait for clock to reach monotonic target
clock.wait_close(target) -> Result<u64>      wait for closed_count >= target
```

clock.wait_open and clock.wait_close follow the same SENTINEL rule as Interlock's
wait_open/wait_close: the loaded word is checked against SENTINEL before it is compared to
target, and expiration_ns is re-checked against SENTINEL (or an already-past expiration)
after every futex wake. Either condition returns InterlockReaped.

The clock carries monotonic time:

- open_count = current monotonic time (updated every daemon cycle, the advancing counter)
- closed_count = daemon start monotonic ms (set once at startup, fixed)
- value = uptime in ms

WaitTimers and WaitCrons watch clock.open_count (the advancing counter). Timer targets are
absolute monotonic timestamps.

</clock-api>

<background-touch>

## Background touch thread

The SDK spawns a background thread per interlock that calls touch() at a fixed interval
(configurable, default below the creation TTL of 100ms). This keeps the interlock alive
without the owner explicitly touching.

```
interlock.start_touch_thread(interval_ms) -> &TouchThread
touch_thread.stop()
interlock.stop_touch_thread(&mut self)
```

stop_touch_thread() drops the current touch thread without terminating the interlock
itself: the interlock stays live until its TTL is next allowed to lapse, or until free() is
called explicitly. It is available on Interlock.

On drop: the thread stops, the heartbeat lapses, the daemon reaps. See Termination, below,
for how this differs from calling free().

</background-touch>

<wait-cron-api>

## WaitCron

A recurring grid-aligned timer. Auto-re-arms after each wake to the next grid line. No UDS
round-trip per cycle. Grid-aligned to monotonic epoch: every Nth millisecond from epoch. No
custom anchor. No phase drag.

```
client.create_wait_cron(name, interval_ms) -> WaitCron
    interval_ms: the grid spacing (aligned to monotonic epoch)

    The daemon auto-re-arms after each wake:
        open_count = next_grid_line(monotonic_epoch, interval, closed_count)
    The client loops futex_wait without re-creating.

cron.wait() -> WaitResult
    Blocks until the next grid line. Returns completed_at.
    On return, the daemon has already re-armed for the next firing.
    Call wait() again immediately to sleep until the next grid line.
    Reaped detection checks closed_count == SENTINEL directly, and separately
    checks expiration_ns == SENTINEL (or an already-past expiration) after each
    futex wake. Either condition returns InterlockReaped.

cron.free(&mut self)                    stops the touch thread, then frees the
                                         interlock (see Termination, below)
```

Use case: six capture workers on isolated cores, each running a WaitCron at 33ms intervals.
All wake within sub-millisecond of each other on every cycle because they share the same
monotonic-epoch-aligned grid. Each does a non-blocking camera buffer check, processes if available, and
calls wait() again.

</wait-cron-api>

<wait-barrier-api>

## WaitBarrier

Wait for all of N conditions to be met. Each condition watches a different interlock at a
different threshold. Daemon-evaluated: one interlock, one check per cycle for all N conditions.

```
client.create_wait_barrier(name, conditions) -> WaitBarrier
    conditions: Vec<(watched_name, watched_word, threshold)>
    Each tuple: which interlock, which word (open or closed), what value to reach.

barrier.wait() -> WaitResult
    Blocks until ALL conditions are met.
    closed_count is stamped to clock.open_count when the barrier fires.
    Reaped detection checks expiration_ns, closed_count, and open_count against
    SENTINEL: once before entering the futex wait, and again (expiration_ns only)
    after each wake.

barrier.free(&mut self)                 stops the touch thread, then frees the
                                         interlock (see Termination, below)
```

Use case: six cameras, each with an interlock tracking capture cycles. WaitBarrier on all six
closed_counts at the same frame number. Fires when all six cameras have completed frame N.

</wait-barrier-api>

<boolean-compositions>

## Boolean compositions (daemon-evaluated)

Interlocks have binary state: Open (value > 0) = 1, Closed (value == 0) = 0. Boolean
operations over N interlocks compose as bit masks.

```
client.create_wait_and(name, watched_names) -> WaitAnd
    Wake when ALL watched interlocks are Open.

client.create_wait_or(name, watched_names) -> WaitOr
    Wake when ANY watched interlock is Open.

client.create_wait_xor(name, watched_names) -> WaitXor
    Wake when exactly one is Open and the rest are Closed.

client.create_wait_nand(name, watched_names) -> WaitNand
    Wake when ALL watched interlocks are Closed.
```

WaitBarrier checks threshold values (has it crossed N?). Boolean compositions check
instantaneous state (is it Open right now?). Both are useful:

- WaitBarrier: "all cameras have completed frame 100" (cumulative progress)
- WaitAnd: "all cameras are currently in-progress" (instantaneous state)
- WaitNand: "all cameras have finished their current work" (all idle)

Boolean compositions (WaitAnd, WaitOr, WaitXor, WaitNand) are planned but not yet
implemented. They require daemon-side tier discriminants not present in the current
wire ABI (v1). They will be added in a wire ABI revision.

</boolean-compositions>

<sdk-compositions>

## SDK-only compositions (no daemon changes)

### WaitRace

First condition met wins. Returns which one fired and when.

```
WaitRace::new(counters: Vec<WaitCounter>) -> WaitRace

race.wait() -> (usize, WaitResult)
    Targets must be set on each counter (via wait_until) before calling
    wait(). Returns when any counter's closed_count advances from its snapshot.
```

No free(): WaitRace is an SDK-only composition, not an interlock type. It owns its
WaitCounters and polls their closed_count on a 1ms sleep loop; it has no touch thread of
its own.

Pre-existing limitation: race.wait() returns (usize, WaitResult), not a Result, so if a
polled counter's closed_count reads as SENTINEL (that counter has been reaped or freed
out from under the race), WaitRace has no way to surface it as an error. A SENTINEL value
is greater than the counter's initial snapshot like any other advance, so the race reports
it as a normal win rather than raising InterlockReaped.

Use case: time to first camera ready.

### WaitUntil

Syntactic sugar for an absolute-time wait.

```
wait_timer.wait_until(timestamp_ms) -> WaitResult
    Equivalent to wait_ms(timestamp_ms - clock.open_count).
```

### ProcessClock

An interlock used as a liveness and uptime beacon. The SDK touch thread updates both
expiration (keepalive) and open_count (current time) on each tick.

```
client.create_process_clock(name) -> ProcessClock
    closed_count = clock.open_count at creation (start time, never changes)
    open_count = updated to clock.open_count on each touch

clock.uptime_ms() -> i64           open_count - closed_count
clock.last_seen_ms() -> u64        open_count (last touch time)
process_clock.free(&mut self)      stops the touch thread, then frees the
                                    interlock (see Termination, below)
```

Other processes attach read-only. A WaitCounter watching a ProcessClock's open_count fires
at a specific uptime threshold ("this process has been running for 60 minutes").

Multiple named process clocks are natural: "clock_pod1_capture", "clock_pod2_inference".
The daemon sees them as ordinary interlocks. The semantics are SDK-side.

</sdk-compositions>

<termination>

## Termination

Every SDK type that owns a background touch thread exposes free(), and Interlock also
exposes stop_touch_thread() (stop the thread without terminating the interlock, see
Background touch, above). free() is an explicit, client-initiated termination: it writes
the same SENTINEL (u64::MAX) value the daemon writes on reap, so waiters see it
immediately rather than waiting out a TTL.

| Type | Method | What it does |
|------|--------|---------------|
| Interlock | free(&mut self) | stops the touch thread, then calls interlock_free (sets expiration_ns to SENTINEL, wakes waiters) |
| AttachedInterlock | free(&self) | writes SENTINEL to open_count, wakes waiters on open_count and closed_count. No write access to expiration_ns (attacher has expiration r/o), so it signals through the counters instead of through interlock_free |
| WaitCounter | free(&mut self) | stops the touch thread, then calls interlock_free |
| WaitTimer | free(&mut self) | stops the touch thread, then calls interlock_free |
| WaitCron | free(&mut self) | stops the touch thread, then calls interlock_free |
| WaitBarrier | free(&mut self) | stops the touch thread, then calls interlock_free |
| ProcessClock | free(&mut self) | stops the touch thread, then calls interlock_free |
| AttachedWaitCounter | none | read-only: no write access to any word, so no free() |
| ClockHandle | none | the system clock is daemon-owned, never terminated by a client |
| WaitRace | none | SDK-only composition, no touch thread of its own (see WaitRace, above) |

free() versus drop: dropping any of Interlock, WaitCounter, WaitTimer, WaitCron,
WaitBarrier, or ProcessClock only stops its touch thread; it does not call
interlock_free. The heartbeat simply lapses and the daemon reaps the interlock on TTL
expiry, exactly as the Background touch section describes for an ordinary drop. free() is
the immediate, explicit path: it writes SENTINEL itself and wakes waiters synchronously,
instead of waiting for the daemon's next TTL-driven reap cycle.

</termination>

<error-summary>

## Error summary

| Error | Source | Fatal | When |
|-------|--------|-------|------|
| InterlockReaped | daemon (TTL reap) or client (free()) | no | TTL lapsed, name claimed, or a word read as SENTINEL (u64::MAX), the terminal marker written by both a daemon reap and a client free() |
| RTSTimeout | SDK (futex timeout) | yes (WaitTimer), no (other) | daemon did not deliver within 2*W |
| RTSOverrun | SDK (value < 0) | no | daemon delivered late (watched value overshot target) |

</error-summary>
