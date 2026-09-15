<purpose>
# docs/

RF-RTS documentation. The LLM kickoff (read-cold entry point) and the architecture-rung design set
that Forge consumes.
</purpose>

<contracts>
## Contracts

- Read `KICKOFF.md` first. It is the cold-start brief for the whole project.
- `design/` is the architecture rung. It is what Forge takes as input to produce the spec.
- HOUSE_STYLE governs every file here. No em or en dashes.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `KICKOFF.md` | LLM entry point. Thesis, the one-primitive-three-contracts model, the daemon and SDK split, the build plan. Read cold. |
</files>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `design/` | The architecture rung: ARCHITECTURE, CONTRACTS, state machines. Forge consumes this. |
</subdirectories>
