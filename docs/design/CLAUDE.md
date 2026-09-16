<purpose>
# design/

The Abacus RTS design rung. Three documents, each answering a different question.
</purpose>

<contracts>
## Contracts

- LIFECYCLE.md is the mechanism: one interlock, four derived states, type-driven daemon behavior.
- CONTRACTS.md is the interface: daemon-to-SDK boundary, SDK-to-user boundary, wire ABI.
- SURFACE.md is the SDK API: what consumers call, what types compose.
- ARCHITECTURE.md is the flowing argument for why this shape. Read it for context, not for contracts.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `LIFECYCLE.md` | The interlock lifecycle: stored properties, derived states, daemon evaluation, type composition. |
| `CONTRACTS.md` | The frozen interface: UDS surface, wire ABI, field semantics, permissions, daemon-SDK boundary. |
| `SURFACE.md` | SDK API surface: interlock/WaitCounter/WaitTimer API, composition patterns (WaitBarrier, WaitRace). |
| `ARCHITECTURE.md` | The flowing argument: one primitive, three contracts, Wildebeest self-reaping, the 1 ms loop. |
</files>

