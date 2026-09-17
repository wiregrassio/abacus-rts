# Abacus: kickoff

LLM entry point. Read cold, build from it. Abacus RTS is the Abacus Real-Time Scheduler: a
coordination primitive daemon. It provides one primitive, the interlock, and reaps interlocks
whose heartbeat expires. Counters and timers are contracts layered over the interlock by the
client SDK. Not motion control, not hard-real-time, not safety-rated.

The decomposed, Forge-consumable form of this document is `design/`. This file is the single
narrative; `design/ARCHITECTURE.md` and `design/CONTRACTS.md` are the addressable rung. When they
drift, design/ is authoritative for structure and this file for intent.

## Thesis

One primitive: the interlock. Three u64 words in shared memory (a memfd), begin, end, heartbeat.
begin and end are futex-waitable. All three are monotonic, non-negative, incremented by arbitrary
amounts, never decremented. That is the entire substrate. Everything Abacus does is expressed in
those three numbers.

The daemon runs a 1 ms best-effort loop. Each cycle it reaps interlocks whose heartbeat TTL has
expired, and wakes waiters whose watched counter has crossed their target. It hands out interlock
file descriptors over a Unix domain socket. That is the whole daemon.

Counter and timer are not new primitives. They are contracts over the interlock:

- Interlock: Abacus reaps it when its heartbeat TTL expires. That is the only thing Abacus does
  to a bare interlock. Raw interlock traffic (begin and end advancing, futex waits) happens
  entirely outside Abacus.
- Counter: an interlock Abacus watches. Abacus wakes waiters when the counter crosses their
  target. The owner sets its own TTL and owns what a timeout means.
- Timer: a counter whose watched interlock is the system clock, with units of milliseconds and a
  futex timeout that is fatal. Abacus sets the timer's TTL from the wait duration.

The satisfying part: all coordination on the device reduces to three u64 numbers per interlock,
two of them futexes, only ever counting up. Time is not special. The system clock is itself an
interlock whose end counter is the millisecond count; a timer is a counter watching it.

## Daemon versus SDK

Abacus is two things that must not bleed into each other.

The Abacus daemon is the systemd binary on its 1 ms loop. It reads heartbeat words, reaps expired
interlocks, wakes counter-crossers, and hands out interlock fds over UDS (create and attach). It
knows nothing about consumers, background threads, or what a timer means at the API level. It sees
only interlocks and heartbeats.

The Abacus client SDK is the library a process links. It owns the ergonomics: attach to the daemon
over UDS, the background touch thread that keeps a heartbeat fresh, the `timer()`, `wait_ms()`, and
`touch_ms()` calls, the futex-timeout to `RTSTimeout` crash logic, and surfacing `InterlockReaped`.
The counter and timer contracts are enforced client-side on top of the raw interlock the daemon
provides.

The rule: the daemon provides interlocks and reaps them; the SDK provides everything that makes an
interlock feel like a timer or a counter. Consumer-specific behavior is SDK. The shared contract
(reap on TTL, wake on cross, fd handout) is daemon. Two SDKs ship: Rust (native core) and Python
(pyo3 binding over the Rust core, so the wait, touch, and crash logic exists once and Python
inherits it).

## The interface, in full

UDS carries exactly two request types:

- create interlock, returns an fd
- attach to interlock, returns an fd

That is all. No command and control. Everything else happens through the interlock via futex, not
through the daemon. Abacus hands out handles; it does not take orders.

Daemon behavioral contract, on the 1 ms best-effort loop:

- reap interlocks whose heartbeat TTL has expired
- wake waiters whose watched counter has crossed their target (a timer is the clock-counter case)

If it is not one of those two behaviors or the two UDS calls, it is not the daemon's concern.

## Contracts and behavior

TTL is monotonic-forward. On a wait or a touch, the new TTL is `max(current, now + duration)`. It
never decrements. There is no separate wake-target-versus-heartbeat conflict, because the
heartbeat word and the wake-target are different u64s: setting a wake target does not touch the
heartbeat, and refreshing the heartbeat does not touch the wake target.

Interlocks are created with a fixed 100 ms TTL. There is no arbitrary creation TTL: on Abacus,
100 ms is a long time, and a longer init TTL is a footgun. A live-but-idle interlock (a paused
capture cycle, an interlock alive but not incrementing) is kept fresh by touch, not by a long
creation TTL.

`touch_ms(N)` raises the TTL to `max(current, now + N)`. Pure heartbeat refresh, no counter
change. It lives on the interlock, so counter and timer inherit it. The SDK's background thread
auto-touches to keep a heartbeat fresh by default; explicit `touch_ms()` is the manual override
for a known-idle period.

A timer's wait couples two numbers into one. `wait_ms(5)` sets the wake target to clock plus 5 ms
and raises the TTL to `now + 2 * wait` (default). The optional TTL override on the wait sets both
the futex-timeout-crash threshold and the interlock TTL increment together, in lockstep, because
they are the same number. A timer can therefore never be reaped while still legitimately waiting.

Death detection is decentralized and falls out of the timer contract. A timer's futex wait has a
timeout of 2 times the specified wait. On wake, the SDK compares: if the watched counter reached
the target, Abacus woke you, proceed. If the futex timed out instead, Abacus did not wake you
within 2 times your interval, so Abacus is dead: the SDK raises `RTSTimeout` and crashes the
process. Higher-level code never sees it. Capture calls `wait_ms(5)`; if the call returns, it was
fine. If Abacus died, the call never returns normally. The blocking wait is the liveness check, the
same fail-safe shape as withholding a PLC pass signal: if it resumed, it was okay. This works only
for timers, because only a timer knows its wake is time-bounded. A counter waiting on a line
counter has no idea when it will cross, so a counter timeout means nothing and the counter owner
decides how to handle it.

## Wildebeest Mode: self-reaping, no detach

The only exit is death: a lapsed TTL, or writing SENTINEL yourself. Stop using an interlock and
its heartbeat lapses and Abacus reaps it. Or call `free()`, which writes SENTINEL to the interlock
and walks away: no detach protocol, no negotiation, no acknowledgment, the daemon sees it on the
next cycle and cleans up like any other reap. Voluntary or accidental, it is the same mechanism.
Abacus never faults and never asks why; it reaps and moves on. Create a named interlock that
already exists and the previous one is reaped and you take over. So a process that restarts before
the reap interval just recreates its interlock and resumes, and nobody is told anything.

The one error a consumer sees from this: `InterlockReaped`. Go to wait on a counter whose backing
interlock was reaped, because your own TTL lapsed or someone claimed your name, and you get
`InterlockReaped`. From the caller's side the two causes are indistinguishable and the response is
the same: recreate and resume, or crash. One error covers both.

## The wait loop

```
loop:
    scan watched counters, wake waiters whose target was crossed
    reap interlocks whose heartbeat TTL has expired
    end = begin                          # cycle complete, Abacus idle-alive
    sleep until the next whole-millisecond boundary (absolute, from a fixed anchor)
    begin += 1                           # cycle start
```

Compensated sleep to the whole-millisecond boundary keeps jitter sub-millisecond and lands events
on integer milliseconds. The next boundary is computed from a fixed anchor, not from post-work
`now`, so a heavy cycle does not drag the phase. begin advances at wake, end catches up at sleep,
so begin greater than end means a cycle is in progress; the reliable death signal is the waiters'
own futex timeouts, not this skew.

`now` in nanoseconds is roughly a ten-cycle operation. A thousand interlocks is a linear scan of a
few thousand u64 reads per millisecond, trivial on an isolated application core. Abacus could run
on an ESP32; the Jetson core it runs on has an order of magnitude more headroom.

## Shared memory stays in the pod

Interlock and buffer memory is memfd, passed by fd over UDS, never attached to a host-global
shared region. Inside a Convoy pod, every interlock and buffer is held by the pod's Convoy daemon
and passed down by fd; Abacus hands the daemon an interlock fd, the daemon passes it to pod
containers. Kill the pod and every fd closes, the kernel reaps the memory, and Abacus sees the
heartbeats lapse and reaps its records. Nothing is shared by name, only by fd, and fds die with
their holders. Any data that leaves a Convoy pod is already a JPEG submitted over HTTP to the
warehouse, so shared memory never crosses the pod boundary.

## v0: build and test

Lift the interlock and the fd handler from the existing Convoy daemon, thin them to this simpler
form, add the 1 ms loop, the registry of watched counters, and the reaper. Compile to a single
standalone Rust binary. Run it as a plain systemd service on the Jetson, unpinned and unisolated,
and build against it.

Core pinning, isolation, and FIFO scheduling are not part of Abacus and not needed for v0. If
Abacus is later pinned to an isolated core running SCHED_FIFO by the host OS, Abacus neither knows
nor cares; that is deployment, non-canonical to Abacus, and recorded elsewhere.

Deliverable: the daemon binary plus a test container that accesses it exactly as any client would,
so the tests prove the client contract, not just internal logic. Prove that timers fire, counters
cross, heartbeats reap, create-over-existing reaps the old one, and a daemon restart wakes nobody
while every waiter times out cleanly.

## Sequencing against Convoy

Convoy is at code and already contains the interlock and the fd handler. This design re-homes the
interlock into Abacus: Convoy depends on Abacus, not the reverse. So freeze this contract (done, in
`design/CONTRACTS.md`), pin Convoy's interlock work, build Abacus by lifting and thinning that
code, then hand the running service to downstream integration work and rework Convoy's
interlock-touching code into Abacus-client code. Convoy loses its in-process interlock and gains a
UDS client, a smaller surface: Convoy becomes a shared-memory data mover, Abacus coordinates.

## Forge cycle

The daemon and the SDK are two artifacts, and the SDK is a client over the daemon. Compose the
daemon architecture first and close all its review rounds (no HIGH finding surviving) before
composing the SDK, whose contract is written from the daemon's ratified surface. Do not feed both
into one cycle: a client artifact composed before its base closes gets discarded (Forge's own
Convoy lesson). Forge composes from `design/ARCHITECTURE.md` and `design/state-machines/`, with
`design/CONTRACTS.md` as the promoted deliverable; it does not build from this KICKOFF. Review runs
as external API calls, never in-context; if the keys or scripts cannot run, stop.

## Start here (next session)

1. Freeze confirmed in `design/CONTRACTS.md`. Read it and `design/ARCHITECTURE.md`.
2. Build the daemon: lift interlock and fd handler from Convoy, add the 1 ms compensated loop, the
   watched-counter registry, the reaper, the two UDS calls. Standalone Rust binary, systemd,
   unpinned.
3. Build the SDK: Rust core (attach, timer, counter, wait_ms, touch_ms, background touch thread,
   RTSTimeout crash, InterlockReaped), pyo3 Python binding over it.
4. Test container: prove the contract from the client side.
5. Hand the running service to downstream integration work; rework Convoy into an Abacus
   client.
