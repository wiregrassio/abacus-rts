# rf-RTOS

Soft-real-time liveness scheduler for containers on NVIDIA Jetson.

**Status:** Pre-implementation design phase.

A userspace coordinator that replaces heartbeat-poll liveness checking with a shared-memory
atomic tick counter and `futex_wait`-based waitable child counters — matching an RTOS-class
tick (10 ms, ±1–2 ms) without pinning, `SCHED_FIFO`, `isolcpus`, or a kernel rebuild. Not
motion control, not hard-real-time, not safety-rated.

Standalone project — no dependency on Convoy or any other rf-workspace repo.

## Design document

The primary design document is [`docs/KICKOFF.md`](docs/KICKOFF.md): thesis and contract,
the v0 build plan, core map, and the FIFO ladder toward tighter timing guarantees.
