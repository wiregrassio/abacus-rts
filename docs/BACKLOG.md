# Abacus RTS backlog

Known issues, deferred work, and things to come back to. Ordered by likely encounter order
(what you'll hit first when integrating).

## Deployment

### systemd service file
The `abacus` binary compiles but has never run as a standalone service. No .service file, no
socket activation, no restart policy. The test harness runs the daemon in a background thread
(same process). Cross-process behavior (real process death, fd cleanup on SIGKILL, orphaned
memfd mappings) is unexercised outside the in-process test harness.

**When:** before any real workload runs on the Jetson.

### Core pinning and SCHED_FIFO
Explicitly excluded from v0 per KICKOFF.md. The daemon runs unpinned and unisolated. If
pinned to an isolated core later, Abacus neither knows nor cares. That is deployment config,
not Abacus.

## Wire protocol

### Attached response does not carry the tier
The Attached wire response carries only the id, not the tier. The SDK cannot enforce
tier-specific permissions on attach. Also affects free()-eligibility: an attacher cannot
verify whether free() via open_count is appropriate for the tier it attached to. Documented
in CONTRACTS.md permission model. Fix requires a wire ABI v2 revision (add tier byte to
the Attached response payload).

**When:** when a third-party consumer attaches to interlocks it did not create.

## SDK

### Python SDK (pyo3 binding)
KICKOFF.md specifies two SDKs: Rust (native core) and Python (pyo3 binding over Rust). The
Python SDK does not exist. The Rust SDK is the complete implementation; Python wraps it.

**When:** when a Python consumer (inference pipeline, orchestrator) needs Abacus coordination.

### WaitRace has no error path
WaitRace::wait() returns `(usize, WaitResult)`, not `Result`. If all polled counters are
reaped (closed_count == SENTINEL), the race loops forever. A SENTINEL counter's closed_count
satisfies `closed > initial` and returns WaitResult with completed_at = SENTINEL, which is
not a meaningful value.

**When:** when WaitRace is used in production with interlocks that can be freed or reaped.

### Boolean compositions not implemented
WaitAnd, WaitOr, WaitXor, WaitNand are designed in SURFACE.md but not implemented. They
require daemon-side tier discriminants not present in wire ABI v1.

**When:** when a consumer needs instantaneous boolean state composition (all open, any open).

## Documentation

### Type composition diagram says "reap on expired/overrun"
In LIFECYCLE.md, the type composition ASCII diagram's `Interlock (type None)` block says
"daemon: reap on expired/overrun." This contradicts the repo's own contract: "daemon reaps
only on expired TTL. Overrun is informational." The daemon does not reap on overrun.

**When:** next doc pass.
