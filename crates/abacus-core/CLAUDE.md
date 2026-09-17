<purpose>
# abacus-core/

Shared types for Abacus RTS. The interlock memory layout, monotonic clock, futex helpers,
and error vocabulary. Both the daemon and the SDK depend on this crate. Depends on libc only.
</purpose>

<files>
## Files

| File | Purpose |
|------|---------|
| `lib.rs` | Module declarations: clock, error, interlock. |
| `clock.rs` | monotonic_now_nanos, expiration_alive, futex_wake, futex_wait, futex_word, sleep_until_nanos. |
| `error.rs` | Condition (InterlockReaped, InterlockNotFound, AllocationFailed, InvalidRequest), TransportError, ProtocolFault, StartupError. |
| `interlock.rs` | Interlock layout (3 x AtomicU64: open_count, closed_count, expiration_ns, 24 bytes), InterlockHandle (Arc-counted RAII munmap), create/open/arm/reap/dup. |
</files>

<contracts>
## Contracts

- The Interlock struct is repr(C), 24 bytes, three u64 words at offsets 0/8/16.
- interlock_arm uses CAS-max: never decrements expiration. Returns InterlockReaped on terminal zero.
- futex_wait/futex_wake operate on the low 32 bits of a u64 word (Linux kernel constraint).
- monotonic_now_nanos aborts on clock failure (unrecoverable).
</contracts>

<dependencies>
## Dependencies

- External: `libc` for memfd_create, ftruncate, mmap, munmap, dup, futex syscalls, and CLOCK_MONOTONIC access.
- Rust std: atomics (AtomicU64, Ordering), OwnedFd, Arc, io::Error.
- No internal (workspace) dependencies. This is the leaf crate.
</dependencies>

<consumed-by>
## Consumed By

- `abacus-daemon`: interlock layout, clock helpers, error types, futex primitives.
- `abacus-client`: interlock layout, clock helpers, error types, futex primitives.
- `abacus-tests`: error types for assertion matching.
</consumed-by>

<data-flow>
## Data Flow

- **In:** memfd_create allocates a 24-byte mapping; interlock_create returns an InterlockHandle wrapping the mapped region.
- **Through:** Atomic loads/stores on the three u64 words (open_count, closed_count, expiration_ns) by daemon and client processes sharing the fd.
- **Out:** futex_wake broadcasts counter changes to waiters; interlock_arm CAS-maxes expiration_ns for keepalive.
</data-flow>
