# RF-RTS design

<intro>

The architecture rung for RF-RTS. This directory is what Forge consumes: it reads the
architecture, generates a spec, then an implementation, then code. Everything here is design,
no executable content. CONTRACTS.md is the frozen interface surface that Convoy builds against
and Fable regenerates from.

</intro>

<primitive>

## The primitive

One primitive: the interlock. Three u64 words in a memfd.

- `begin`: futex-waitable, monotonic, incremented when work starts or when RF-RTS begins a cycle.
- `end`: futex-waitable, monotonic, incremented when work completes or when RF-RTS ends a cycle.
- `heartbeat`: a CLOCK_MONOTONIC nanosecond timestamp. Future means alive, past means the owner
  is gone. Reaped by RF-RTS when it falls into the past.

All monotonic, non-negative, never decremented. Counter and timer are contracts over this
primitive, not new structures.

</primitive>

<contracts>

## Three contracts

- **Interlock.** RF-RTS reaps it when the heartbeat TTL expires. begin and end advancing, and
  futex waits on them, happen entirely outside RF-RTS.
- **Counter.** An interlock RF-RTS watches. RF-RTS wakes waiters when the counter crosses their
  target. The owner declares its own TTL and owns what a timeout means.
- **Timer.** A counter whose watched interlock is the system clock, units milliseconds, futex
  timeout fatal (`RTSTimeout`). The 2 times W futex timeout is the death signal: if RF-RTS did
  not wake you in time, it is dead, and the SDK crashes the process.

</contracts>

<daemon-and-sdk>

## Daemon and SDK

The daemon is the systemd binary. Its 1 ms best-effort loop reaps expired interlocks and wakes
counter-crossers. Two UDS calls: create and attach, both return an interlock fd. It knows nothing
about consumers or what a timer means; it sees interlocks and heartbeats.

The SDK is the linked client library. It owns attach-over-UDS, the background touch thread,
`timer()`, `wait_ms()`, `touch_ms()`, the futex-timeout to `RTSTimeout` crash, and surfacing
`InterlockReaped`. Counter and timer contracts are enforced here, on top of the raw interlock. Rust
core, pyo3 Python binding over it, so the logic exists once.

Boundary rule: the daemon provides interlocks and reaps them; the SDK provides everything that makes
an interlock feel like a timer or counter.

</daemon-and-sdk>

<data-flow>

## Data flow

A creator calls the daemon over UDS, receives an interlock fd, and uses begin, end, and heartbeat
directly. The daemon advances the system-clock interlock each loop (begin at wake, end at sleep)
and wakes counter waiters whose target was crossed. Nothing is shared by name, only by fd: memfd
passed over UDS through SCM_RIGHTS, never attached to a host-global region and never requiring a
shared IPC namespace. fd passing crosses namespaces on its own, so shared memory stays in the pod.

</data-flow>

<known-hazards>

## Known hazards

- LOW: `begin > end` on any interlock means a cycle or operation is in progress, not that the
  owner died. Death is detected by the waiter's own futex timeout (timers) or is the counter
  owner's concern, never by reading begin and end alone.
- LOW: create-over-existing-name silently reaps the prior interlock (Wildebeest Mode). A slow
  prior owner learns this only on its next touch or wait, as `InterlockReaped`. Intended, not a
  fault.

</known-hazards>

<reading-order>

## Reading order

1. `ARCHITECTURE.md` for the model: one primitive, three contracts, the system clock, Wildebeest
   self-reaping, the 1 ms loop.
2. `CONTRACTS.md` for the frozen interface: the two UDS calls, the reap and wake behavior, TTL
   rules, the daemon and SDK boundary.
3. `state-machines/` for the transition-level detail: interlock, system-clock, counter-wait.

</reading-order>
