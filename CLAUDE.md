<purpose>
# abacus-rts/

Abacus RTS: lockless atomic coordination for real-time compute. One primitive (the interlock),
five daemon-evaluated tiers, SDK compositions. Linux-only (memfd, futex, UDS with SCM_RIGHTS).
</purpose>

<contracts>
## Contracts

- Daemon provides interlocks and evaluates them. SDK provides typed handles and compositions.
- The daemon reaps only on expired TTL. Overrun is informational.
- All interlocks are named. No unnamed interlocks. Attach by name only.
- The "clock" name is reserved (daemon-owned monotonic clock).
- Wire protocol v1: length-prefixed binary frames, SCM_RIGHTS fd passing, 1 fd per response.
- Linux-only. No em or en dashes. Shortest-complete register.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root. Members: abacus-core, abacus-daemon, abacus-client, abacus-tests. |
| `README.md` | Project overview, tagline, reading order. |
| `.gitignore` | Excludes target/ and forge/. |
</files>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `crates/` | Rust workspace crates: core, daemon, client SDK, integration tests. |
| `docs/` | Design documents: KICKOFF (cold-start), design/ (LIFECYCLE, CONTRACTS, SURFACE, ARCHITECTURE). |
</subdirectories>
