# RF-RTS state machines

Normative lifecycle specifications for the three RF-RTS components. Each file is one Mermaid
`stateDiagram-v2`, a transition table, and an internal-state table. The interlock is the sole
primitive; the other two specialize it.

<convention>

## Convention

One component, one diagram. States are named for what the component is doing; transitions carry
their trigger and guard; the prose table matches the diagram exactly. The three machines share one
substrate, the interlock (begin, end, heartbeat), so they read together: the clock is the interlock
the daemon advances, and the counter wait is any waiter blocking on an interlock's counter, with
the timer being the counter wait against the clock.

</convention>

<contract>

## Shared contract

- begin and end advance with release semantics; readers acquire-load. Monotonic, non-negative,
  arbitrary increment, never decremented.
- heartbeat is a CLOCK_MONOTONIC ns timestamp, a distinct word. Future is alive, past is reapable.
- TTL is monotonic-forward: `max(current, now + duration)` on any wait or touch.
- Reaping writes the interlock out of the daemon's registry; a subsequent wait or touch by a stale
  owner raises `InterlockReaped`.
- create-named that exists reaps the prior interlock first.

</contract>

<symbols>

## Symbols

| Symbol | Meaning |
|--------|---------|
| `begin`, `end` | The two futex-waitable counters of an interlock. |
| `heartbeat` | The interlock's liveness timestamp word. |
| `target` | The counter value a waiter is waiting for the watched interlock to cross. |
| `ttl` | The heartbeat horizon; `now + duration`, monotonic-forward. |
| `clock` | The system-clock interlock, whose `end` is the millisecond count. |
| `W` | A timer's specified wait duration in ms. |

</symbols>

<files>

## Files

| File | Component |
|------|-----------|
| `interlock.md` | The sole primitive: begin, end, heartbeat, reaping, create-over-existing. |
| `system-clock.md` | The interlock the daemon advances each loop; what timers watch. |
| `counter-wait.md` | A waiter blocking on a watched counter; the timer is the counter-on-clock case. |

</files>
