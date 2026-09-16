<purpose>
# abacus-tests/

Integration tests for Abacus RTS. Tests start a live daemon in a background thread and exercise
the SDK against it. Each test gets a unique socket path for parallel execution.
</purpose>

<files>
## Files

| File | Purpose |
|------|---------|
| `src/lib.rs` | Test harness: start_daemon, test_socket_path, wait_for_socket, cleanup. |
| `tests/integration.rs` | Integration tests covering timers, counters, reaping, Wildebeest, clock, ProcessClock, permissions. |
</files>

<contracts>
## Contracts

- Tests prove the client contract, not internal daemon logic.
- Each test starts its own daemon thread with a unique socket path.
- Tests require Linux (memfd, futex, UDS). They do not run on macOS.
</contracts>
