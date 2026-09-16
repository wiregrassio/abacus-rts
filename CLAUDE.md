<purpose>
# abacus-rts/

Abacus RTS: the Abacus Real-Time Scheduler. A coordination primitive daemon. It provides one
primitive, the interlock, and reaps interlocks whose heartbeat has expired. Counters and timers
are contracts layered over the interlock, provided by the client SDK. Runs as a systemd service,
zero dependencies but systemd. Standalone; other systems depend on it.
</purpose>

<contracts>
## Contracts

- `docs/KICKOFF.md` is the LLM entry point. Read it cold before touching anything else.
- Daemon versus SDK: the daemon provides interlocks and reaps them; the SDK provides everything
  that makes an interlock feel like a timer or counter. Consumer-specific behavior is SDK.
- If it is not part of the daemon interface or its contract, it does not belong in this repo.
  Deployment topology, core pinning, and consumer cascades are outside Abacus and not recorded here.
- Linux-only: requires memfd, futex, and UDS with SCM_RIGHTS.
- No em or en dashes anywhere. Shortest-complete register.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root manifest. Members: abacus-core, abacus-daemon, abacus-client. |
| `README.md` | Project overview and pointer to the primary design document. |
</files>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `crates/abacus-core/` | Core library: interlock primitive, clock, error types. Depends on libc only. |
| `crates/abacus-daemon/` | Daemon binary: the 1 ms loop, registry, UDS transport, wire protocol. Depends on abacus-core and libc. |
| `crates/abacus-client/` | Client SDK placeholder. Depends on abacus-core and libc. |
| `docs/` | Design documents: the LLM kickoff, the architecture-rung design set, and research. |
</subdirectories>

<source-map>
## Source map

### abacus-core (crates/abacus-core/src/)

| File | Purpose |
|------|---------|
| `lib.rs` | Module declarations: clock, error, interlock. |
| `error.rs` | Condition, TransportError, ProtocolFault, StartupError. |
| `clock.rs` | monotonic_now_nanos, expiration_alive, futex_wake, sleep_until_nanos. |
| `interlock.rs` | Interlock layout (3 x AtomicU64: open_count, closed_count, expiration_ns, 24 bytes), InterlockHandle, memfd create/open/arm/reap/dup. |

### abacus-daemon (crates/abacus-daemon/src/)

| File | Purpose |
|------|---------|
| `main.rs` | Binary entry point: parse --socket-path, call daemon_run. |
| `lib.rs` | Module declarations: daemon, registry, transport, wire. |
| `registry.rs` | Named interlock registry with Tier (Interlock/WaitCounter/WaitTimer), system clock, Wildebeest create-over-existing, evaluate_all (unified reap + wake). |
| `wire.rs` | Two-request wire protocol: CreateInterlock, AttachInterlock. Length-prefixed binary framing. |
| `transport.rs` | UDS server (non-blocking accept), Connection with SCM_RIGHTS fd passing. |
| `daemon.rs` | daemon_run: the 1 ms compensated loop. Update clock (monotonic ms), evaluate_all (reap + wake), serve UDS, sleep. |

### abacus-client (crates/abacus-client/src/)

| File | Purpose |
|------|---------|
| `lib.rs` | Placeholder. Will provide WaitTimer, WaitCounter, background touch thread, RTSTimeout/RTSOverrun. |
</source-map>
