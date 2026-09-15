# RF-RTS design

The architecture rung for RF-RTS. This directory is what Forge consumes: it reads the
architecture, generates a spec, then an implementation, then code. Everything here is design, no
executable content. CONTRACTS.md is also the freeze that Convoy builds against and Fable
regenerates from.

<the-primitive>

## The primitive

One primitive: the interlock. Three u64 words in a memfd.

- `begin`: futex-waitable, monotonic, incremented when work starts or when RF-RTS begins a cycle.
- `end`: futex-waitable, monotonic, incremented when work completes or when RF-RTS ends a cycle.
- `heartbeat`: a CLOCK_MONOTONIC nanosecond timestamp. Future means alive, past means the owner is
  gone. Reaped by RF-RTS when it falls into the past.

All monotonic, non-negative, incremented by arbitrary amounts, never decremented. Counter and
timer are contracts over this, not new structures.

</the-primitive>

<three-contracts>

## Three contracts

- Interlock. RF-RTS reaps it when the heartbeat TTL expires. That is the only thing RF-RTS does to
  a bare interlock. begin and end advancing, and futex waits on them, happen entirely outside
  RF-RTS. A consumer using an interlock only for its own coordination never involves the daemon
  beyond create, attach, and reaping.
- Counter. An interlock RF-RTS watches. RF-RTS wakes waiters when the counter crosses their target.
  The owner declares its own TTL and owns what a timeout means, because nothing can bound when an
  arbitrary counter will cross.
- Timer. A counter whose watched interlock is the system clock, units milliseconds, futex timeout
  fatal. RF-RTS sets the TTL from the wait duration. This is the only contract where a timeout is a
  death signal, because only a timer knows its wake is time-bounded.

</three-contracts>

<system-clock>

## The system clock is an interlock

Time is not special. The system clock is an interlock whose `end` counter is the millisecond
count. RF-RTS increments it on its own loop: `begin` at wake, `end` at sleep. A timer is a counter
watching this interlock's counter; "wake me in 5 ms" is "wake me when the clock's counter crosses
now plus 5." The same watch-and-wake machinery serves time and progress; only the watched
interlock differs.

Because RF-RTS advances the clock's begin at wake and end at sleep, `begin > end` means a cycle is
in progress. This is not the death signal (a single read cannot distinguish in-progress from
died-mid-cycle). The reliable death signal is decentralized: each timer's own futex timeout.

</system-clock>

<daemon-and-sdk>

## Daemon and SDK

RF-RTS is two parts.

The daemon is the systemd binary. Its 1 ms loop reaps expired interlocks and wakes counter-crossers.
It hands out interlock fds over UDS through two calls, create and attach. It knows nothing about
consumers or what a timer means; it sees interlocks and heartbeats.

The SDK is the linked client library. It owns attach-over-UDS, the background touch thread, the
`timer`, `wait_ms`, `touch_ms` calls, the futex-timeout to `RTSTimeout` crash, and surfacing
`InterlockReaped`. The counter and timer contracts are enforced here, on top of the raw interlock.
Two SDKs: Rust core, and a pyo3 Python binding over it so the logic exists once.

The boundary rule: the daemon provides interlocks and reaps them; the SDK provides everything that
makes an interlock feel like a timer or counter. Consumer-specific behavior is SDK; the shared
contract is daemon.

</daemon-and-sdk>

<ttl-and-touch>

## TTL and touch

TTL is monotonic-forward: on any wait or touch, TTL becomes `max(current, now + duration)`, never
less. The heartbeat word and the wake-target are different u64s, so setting a wake target does not
disturb the heartbeat and refreshing the heartbeat does not disturb the wake target; the old
"cannot decrement a future TTL" concern does not arise.

Interlocks are created with a fixed 100 ms TTL. There is no arbitrary creation TTL: on RF-RTS
100 ms is long, and a longer init TTL invites misuse. A live-but-idle interlock, such as a paused
capture cycle that is alive but not incrementing, is kept fresh with touch.

`touch_ms(N)` raises TTL to `max(current, now + N)` with no counter change. It lives on the
interlock, so counter and timer inherit it. The SDK's background thread auto-touches to keep a
heartbeat fresh; explicit `touch_ms` is the manual override for a known-idle stretch.

A timer's `wait_ms(5)` couples the wake target and the TTL: it sets the wake target to clock plus
5 ms and raises TTL to `now + 2 * wait`. The optional TTL override sets the futex-timeout crash
threshold and the interlock TTL together, one number, so a timer can never be reaped while
legitimately waiting.

</ttl-and-touch>

<death-detection>

## Death detection

Decentralized, emergent, no central health check. A timer's futex wait times out at 2 times the
specified wait. On wake the SDK compares: watched counter reached target means RF-RTS woke you,
proceed; futex timed out means RF-RTS did not wake you within 2 times your interval, so RF-RTS is
dead, and the SDK raises `RTSTimeout` and crashes the process. Higher-level code never sees it: the
blocking wait is the liveness check, and if the call returned, it was fine. This is the same
fail-safe shape as withholding a downstream pass signal.

Only timers get this, because only a timer's wake is time-bounded. A counter waiting on a progress
counter cannot interpret a timeout, so the counter owner decides.

</death-detection>

<wildebeest>

## Wildebeest Mode: self-reaping

No drop, no invalidate, no explicit detach. Stop using an interlock and its heartbeat lapses and
RF-RTS reaps it. RF-RTS never faults; it reaps and moves on. Create a named interlock that exists
and the previous one is reaped and you take over, so a process restarting before the reap interval
recreates its interlock and resumes with nobody told.

The single consumer-visible error from this is `InterlockReaped`: wait on a counter whose backing
interlock was reaped, whether your own TTL lapsed or someone claimed your name, and you get it. The
two causes are indistinguishable to the caller and the response is the same, recreate and resume or
crash, so one error covers both. This deletes an entire category of teardown, ownership, and
stale-handle code.

</wildebeest>

<the-loop>

## The wait loop

Each cycle: scan watched counters and wake waiters whose target was crossed; reap interlocks whose
heartbeat expired; set `end = begin`; sleep to the next whole-millisecond boundary computed from a
fixed anchor; increment `begin`. Compensated sleep keeps jitter sub-millisecond and lands events on
integer milliseconds, and anchoring the next boundary rather than measuring from post-work `now`
keeps a heavy cycle from dragging the phase.

The scan is a linear pass over the registered interlocks, a few thousand u64 reads per millisecond
for a thousand interlocks, trivial on an application core. RF-RTS holds no durable state, so a
restart returns empty, wakes no one, and is discovered by every waiter's own timeout: a cold
restart is correct behavior, not recovery.

</the-loop>

<shared-memory>

## Shared memory stays in the pod

Interlock memory is memfd, passed by fd over UDS through SCM_RIGHTS, never attached to a
host-global region and never requiring a shared IPC namespace. fd passing crosses namespaces on its
own. Inside a Convoy pod the Convoy daemon holds the interlock fds RF-RTS hands it and passes them
to pod containers; killing the pod closes every fd, the kernel reaps the memory, and RF-RTS reaps
its records when the heartbeats lapse. Nothing is shared by name, only by fd, and fds die with
their holders. Data leaving a pod is already a JPEG over HTTP to the warehouse, so shared memory
never crosses the pod boundary.

</shared-memory>

<out-of-scope>

## Out of scope

RF-RTS knows nothing about who uses it, how they are scheduled, or what happens when they die. Core
pinning, isolation, SCHED_FIFO placement, the Convoy pod layout, and any fail-safe cascade are
deployment and consumer concerns, non-canonical to RF-RTS, and are not recorded in this repo. If
RF-RTS is pinned to an isolated core by Roboflow OS, RF-RTS neither knows nor cares.

</out-of-scope>
