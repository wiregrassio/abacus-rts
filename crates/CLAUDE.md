<purpose>
# crates/

Rust workspace crates. One crate per concern: core types, daemon binary, client SDK,
integration tests.
</purpose>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `abacus-core/` | Shared types: interlock primitive, clock, error. Depends on libc only. |
| `abacus-daemon/` | Daemon binary: poll()-based 1ms event loop, registry, wire protocol, transport. |
| `abacus-client/` | Client SDK: typed handles, wait primitives, touch threads, compositions. |
| `abacus-tests/` | Integration tests: daemon + SDK exercised together against the client contract. |
</subdirectories>
