# RF-RTS contracts

The frozen interface surface. This is what Convoy builds against and what Fable regenerates from.
Anything not here is not part of RF-RTS. The organizing test: if it is not an interface or a
contract, it does not belong.

<uds-surface>

## UDS surface

The daemon exposes a Unix domain socket carrying exactly two request types. No command and
control.

| Request | Returns | Meaning |
|---------|---------|---------|
| create interlock | fd | Create a new interlock (optionally named), initialize it, return its memfd. |
| attach interlock | fd | Attach to an existing interlock by name or id, return its memfd. |

Everything else happens through the interlock via futex, not through the daemon. Create-named that
already exists reaps the previous interlock and returns the new one (Wildebeest Mode).

</uds-surface>

<interlock-shape>

## Interlock shape

Three u64 words in a memfd:

| Word | Futex-waitable | Semantics |
|------|----------------|-----------|
| `begin` | yes | Monotonic, non-negative, arbitrary increment. Work started, or RF-RTS cycle started. |
| `end` | yes | Monotonic, non-negative, arbitrary increment. Work completed, or RF-RTS cycle ended. |
| `heartbeat` | no | CLOCK_MONOTONIC ns. Future = alive, past = reapable. Distinct word from begin and end. |

Never decremented. `begin` and `end` are independent of `heartbeat`: a wake-target set on a counter
does not touch `heartbeat`, and a touch does not touch begin or end.

</interlock-shape>

<daemon-behavior>

## Daemon behavior (1 ms best-effort loop)

The daemon guarantees, on a best-effort 1 ms loop:

- Reap: any interlock whose `heartbeat` is in the past is reaped.
- Wake: any waiter whose watched counter has crossed its registered target is woken.

Those two behaviors and the two UDS calls are the entire daemon contract. The daemon makes no
claim about consumers, ordering beyond the loop, or what a woken process should do.

</daemon-behavior>

<ttl-rules>

## TTL rules

- Creation TTL is fixed at 100 ms. No arbitrary creation TTL is offered.
- TTL is monotonic-forward: any wait or touch sets TTL to `max(current, now + duration)`. Never
  decrements.
- `touch_ms(N)` raises TTL to `max(current, now + N)` with no counter change. Available on any
  interlock; inherited by counter and timer.
- A timer wait couples wake-target and TTL: `wait_ms(W)` sets wake-target to clock + W and TTL to
  `now + 2 * W` by default. The optional TTL override on the wait sets the futex-timeout crash
  threshold and the interlock TTL together as one number.

</ttl-rules>

<contract-tiers>

## The three contract tiers

| Tier | RF-RTS involvement | TTL | Timeout meaning |
|------|--------------------|-----|-----------------|
| Interlock | reap on expired heartbeat only | owner sets, default 100 ms, touch to extend | none; raw futex use is the owner's |
| Counter | reap, plus wake on counter crossing target | owner declares | owner decides; RF-RTS makes no claim |
| Timer | reap, plus wake on clock crossing target | set from wait, `2 * W` default | fatal: `RTSTimeout`, crash the process |

A timer is the counter tier with the watched interlock fixed to the system clock, units ms, and
timeout fatal.

</contract-tiers>

<errors>

## Errors

| Error | When | Response |
|-------|------|----------|
| `InterlockReaped` | Wait or touch on an interlock that was reaped (own TTL lapsed, or name claimed by another create). | Recreate and resume, or crash. The two causes are indistinguishable and the response is the same. |
| `RTSTimeout` | A timer's futex wait timed out (2 x wait) instead of being woken: RF-RTS did not wake in time, so it is dead. | Fatal. The SDK crashes the process. Higher-level code does not handle it. |

</errors>

<daemon-sdk-boundary>

## Daemon and SDK boundary

| Concern | Owner |
|---------|-------|
| create and attach over UDS, returning fds | daemon |
| reap on expired heartbeat | daemon |
| wake on counter crossing target | daemon |
| the 1 ms loop and the system-clock interlock | daemon |
| attach ergonomics, handle lifetime | SDK |
| background touch thread (auto-keepalive) | SDK |
| `timer()`, `wait_ms()`, `touch_ms()` | SDK |
| futex-timeout to `RTSTimeout` crash | SDK |
| surfacing `InterlockReaped` | SDK |
| enforcing the counter and timer contracts over the raw interlock | SDK |

Rule: consumer-specific behavior is SDK; the shared primitive contract is daemon. Two SDKs, Rust
core and pyo3 Python binding over it, so the wait, touch, and crash logic exists once.

</daemon-sdk-boundary>

<explicitly-excluded>

## Explicitly excluded

Not part of RF-RTS, by the interface-or-contract test: core pinning, CPU isolation, SCHED_FIFO
placement, deployment topology, the Convoy pod layout, any consumer fail-safe cascade, and any
knowledge of who consumes RF-RTS. These are deployment and consumer concerns, recorded outside this
repo if at all.

</explicitly-excluded>
