<purpose>
# abacus-client/

Abacus client SDK. Typed handles for interlocks with type-level permission enforcement.
Connects to the daemon over UDS, creates/attaches interlocks, provides wait primitives
with timeout detection and background keepalive.
</purpose>

<files>
## Files

| File | Purpose |
|------|---------|
| `lib.rs` | Module declarations and re-exports. |
| `client.rs` | AbacusClient: connect, create/attach methods for all tiers, clock handle, is_connected. |
| `types.rs` | WaitState, WaitResult, InterlockState, WatchedWord, RTS_TIMEOUT, RTS_OVERRUN, default constants. |
| `interlock.rs` | Interlock (creator r/w), AttachedInterlock (attacher r/w counters, r/o expiration), AttachedWaitCounter (r/o), ClockHandle (r/o). |
| `wait_counter.rs` | WaitCounter: wait_until(target, timeout_ms), CAS-max on open_count, reaped detection. |
| `wait_timer.rs` | WaitTimer: wait_ms with absolute 2*W deadline, RTSTimeout abort, wait_until sugar. CAS-max on open_count. |
| `wait_cron.rs` | WaitCron: recurring grid-aligned wait, snapshot-based wake detection. |
| `wait_barrier.rs` | WaitBarrier: multi-condition wait, daemon-evaluated. |
| `wait_race.rs` | WaitRace: first-of-N polling over WaitCounters. SDK-only, no daemon changes. |
| `process_clock.rs` | ProcessClock: liveness + uptime beacon. Custom touch thread updates open_count to current time. |
| `touch.rs` | TouchThread: background keepalive at configurable interval (default 40ms). Reaped flag. |
</files>

<contracts>
## Contracts

- Permission model is type-level: the compiler prevents calling write methods on read-only types.
- All wait paths check for reaped state and return Err(InterlockReaped).
- WaitTimer timeout is an absolute deadline (not per-iteration). RTSTimeout aborts the process.
- CAS-max on open_count: never writes backward. Concurrent calls preserve the largest target.
- Touch thread refreshes expiration at 40ms intervals (2.5 ticks per 100ms TTL).
- Known coupling: depends on abacus-daemon for wire codec types. Extract to shared crate is planned.
</contracts>

<dependencies>
## Dependencies

- Internal: `abacus-core` for interlock layout, clock primitives, error types, futex helpers. `abacus-daemon` for wire codec types (Request/Response enums, encode/decode).
- External: `libc` for UDS connect, SCM_RIGHTS recv, futex_wait.
- Rust std: io, os::unix, net, thread, sync (Arc, atomic), time.
</dependencies>

<consumed-by>
## Consumed By

- `abacus-tests`: the test suite exercises the full SDK surface against a live daemon.
- Downstream consumers (Convoy, application code): link this crate to coordinate via Abacus.
</consumed-by>

<data-flow>
## Data Flow

- **In:** AbacusClient::connect opens a UDS to the daemon, sends Create/Attach requests, receives responses with fds via SCM_RIGHTS.
- **Through:** Typed handles (Interlock, WaitCounter, WaitTimer, WaitCron, WaitBarrier, ProcessClock) wrap the mapped interlock memory. Background TouchThread refreshes expiration_ns at 40ms intervals.
- **Out:** Wait primitives (wait_ms, wait_until, wait) block on futex_wait against interlock words. Returns WaitResult with completed_at and WaitState. RTSTimeout aborts the process for timer failures.
</data-flow>

<known-hazards>
## Known Hazards

- **MEDIUM:** Wire codec coupling. The client imports Request/Response types directly from `abacus-daemon::wire`. A daemon wire format change forces a client rebuild. Planned fix: extract wire types to `abacus-core` or a dedicated `abacus-wire` crate.
</known-hazards>
