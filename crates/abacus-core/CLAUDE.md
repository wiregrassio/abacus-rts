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
