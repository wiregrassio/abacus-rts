<purpose>
# design/

The RF-RTS architecture rung. The design as flowing argument, the frozen interface contract, and
the component state machines. This is the input Forge concretizes into a spec, then an
implementation, then code. The contract here is what Convoy builds against and Fable regenerates
from.
</purpose>

<contracts>
## Contracts

- ARCHITECTURE is the model (flowing argument). CONTRACTS is the frozen interface surface. The
  state machines are the transition-level detail. They must stay transition-equivalent.
- The interlock is the sole primitive. Counter and timer are contracts over it, enforced by the
  SDK. State machines describe the interlock, the system clock (a special interlock), and the
  watched-counter wait.
- Daemon versus SDK: the daemon provides interlocks and reaps them; the SDK provides the timer
  and counter ergonomics. CONTRACTS states the boundary.
- If it is not part of the daemon interface or a contract, it does not belong here. No deployment,
  no core map, no consumer cascade.
- Read `README.md` before modifying any file here.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `README.md` | Comprehension doc: the primitive, the daemon and SDK split, data flow, hazards, reading order. |
| `ARCHITECTURE.md` | The model. One primitive, three contracts, the system clock, Wildebeest self-reaping, the 1 ms loop. Flowing argument. |
| `CONTRACTS.md` | The frozen interface: the two UDS calls, the reap and wake behavior, TTL rules, the daemon and SDK boundary. What Convoy builds against. |
</files>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `state-machines/` | One Mermaid state diagram per component: interlock, system clock, counter wait. |
</subdirectories>
