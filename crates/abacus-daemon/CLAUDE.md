<purpose>
# abacus-daemon/

Abacus daemon binary. A poll()-based 1ms event loop that evaluates every interlock each cycle:
reap expired, stamp and wake watched interlocks whose conditions are met, advance the system
clock. Persistent UDS connections with SCM_RIGHTS fd passing.
</purpose>

<files>
## Files

| File | Purpose |
|------|---------|
| `lib.rs` | Module declarations: daemon, registry, transport, wire. |
| `main.rs` | Binary entry point: parse --socket-path, call daemon_run. Binary name: `abacus`. |
| `daemon.rs` | daemon_run: poll()-based event loop. Connection table, POLLIN/POLLHUP handling, clock advancement, evaluate_all dispatch. |
| `registry.rs` | Interlock registry: Tier enum (Interlock, WaitCounter, WaitTimer, WaitCron, WaitBarrier), evaluate_all (single-pass reap + wake), Wildebeest create-over-existing, clock as named "clock" entry. |
| `wire.rs` | Wire protocol v1: Request/Response enums, encode/decode, tier-specific payloads for all 5 tiers. |
| `transport.rs` | UDS server (non-blocking accept), Connection (send/recv with SCM_RIGHTS), WouldBlock handling. |
</files>

<contracts>
## Contracts

- The daemon evaluates every interlock every 1ms cycle (best-effort).
- Daemon checks SENTINEL on any word before TTL. If any word is SENTINEL, reap and remove.
- Reap only on expired TTL. Overrun is informational, not daemon-fatal.
- The "clock" name is reserved. create("clock") is rejected.
- One fd per response. The clock is attached separately by name.
- Persistent connections: clients connect once, send many requests.
- WouldBlock on a client fd: skip this cycle, keep connection.
- Connection death (EOF/EPIPE): remove client from table, no reap.
</contracts>

<dependencies>
## Dependencies

- Internal: `abacus-core` for interlock layout, clock primitives, error types, futex helpers.
- External: `libc` for poll(), socket operations, SCM_RIGHTS ancillary data, signal handling.
- Rust std: io, os::unix, net, thread, time.
</dependencies>

<consumed-by>
## Consumed By

- `abacus-client`: imports wire codec types (Request, Response enums) for encode/decode. This is a known coupling; the wire types should live in a shared crate.
- `abacus-tests`: starts daemon_run in a background thread for integration testing.
</consumed-by>

<data-flow>
## Data Flow

- **In:** UDS connections arrive via non-blocking accept. Framed binary requests (Create, Attach) decoded from each connection.
- **Through:** Registry maps names to interlock entries with tier metadata. Each 1ms cycle: advance the clock interlock, evaluate_all (single pass, loads all three words once and reuses them: checks SENTINEL on any word first, reaps and removes on a hit, otherwise checks TTL and stamps/wakes watched interlocks whose conditions are met).
- **Out:** Created/Attached responses with SCM_RIGHTS fd passing back to clients. Futex wakes on interlock words for cross-process signaling.
</data-flow>
