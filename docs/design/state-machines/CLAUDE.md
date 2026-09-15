<purpose>

# state-machines/

Normative state-machine specifications for the RF-RTS interlock, the system clock, and the
watched-counter wait. The interlock is the sole primitive; the system clock and the counter wait
are its specializations. Timer is the counter wait with the watched interlock fixed to the clock.

</purpose>

<dependencies>

## Dependencies
- `../ARCHITECTURE.md`: the one-primitive-three-contracts model, the daemon and SDK split.
- `../CONTRACTS.md`: the frozen interface these machines must not exceed.
- Mermaid `stateDiagram-v2`: diagram notation. No executable imports; design documentation.

</dependencies>

<data-flow>

## Data Flow
- A creator calls the daemon over UDS, receives an interlock fd, and uses begin, end, and heartbeat
  directly. RF-RTS reaps the interlock when its heartbeat lapses.
- The daemon advances the system-clock interlock every loop: begin at wake, end at sleep.
- A counter waiter registers a target against a watched interlock and blocks on a futex; the daemon
  wakes it when the counter crosses the target, or the waiter's own timeout fires.

</data-flow>

<known-hazards>

## Known Hazards
- LOW: `begin > end` on any interlock means a cycle or operation is in progress, not that the owner
  died. Death is detected by the waiter's own futex timeout (timers) or is the owner's concern
  (counters), never by reading begin and end alone.
- LOW: create-over-existing silently reaps the prior interlock. A slow prior owner learns this only
  on its next touch or wait, as `InterlockReaped`. This is intended (Wildebeest Mode), not a fault.

</known-hazards>

<notes>

## Notes
- `interlock.md` is foundational: begin, end, heartbeat, reaping, and create-over-existing are
  reused by the clock and the counter wait.
- `system-clock.md` is the interlock the daemon owns and advances; it is what timers watch.
- `counter-wait.md` covers both the general counter and the timer (counter-on-clock) case.

</notes>

<operator-notes>

## Operator notes

One Mermaid diagram per component, kept in step with `../ARCHITECTURE.md` and `../CONTRACTS.md`. A
transition present in prose and absent in the diagram, or the reverse, is a defect in one of them.

</operator-notes>
