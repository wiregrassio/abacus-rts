# Abacus contracts

The frozen interface surface. Anything not here is not part of Abacus. The organizing test: if
it is not an interface or a contract, it does not belong. LIFECYCLE.md documents how the
daemon works internally. SURFACE.md documents the SDK API. This file documents the boundaries
between them.

<uds-surface>

## UDS surface

The daemon exposes a Unix domain socket carrying exactly two request types. No command and
control.

| Request | Fields | Returns | Meaning |
|---------|--------|---------|---------|
| create | name, tier, watched_name?, watched_word? | 1 fd | Create a named interlock with a tier. Return its memfd. |
| attach | name | 1 fd | Attach to an existing interlock by name. Return its memfd. |

One fd per response. The clock is a well-known named interlock "clock"; clients attach to it by
name. Everything else happens through the interlock via futex, not through the daemon.
Create-named that already exists reaps the previous interlock and returns the new one (Wildebeest
Mode). The name "clock" is reserved; create("clock") is rejected.

</uds-surface>

<create-call>

## Create call

```
create(name: String, tier: Tier, watched_name?: String, watched_word?: u8,
       interval_ns?: u64, barrier_conditions?: [(String, u8, u64)]) -> fd
```

| Tier | Daemon behavior | Create fields | Auto-re-arm |
|------|-----------------|---------------|-------------|
| Interlock (0) | reap only | name | no |
| WaitCounter (1) | reap + watch | name, watched_name (must exist), watched_word (0/1) | no |
| WaitTimer (2) | reap + watch clock | name | no |
| WaitCron (3) | reap + watch clock + auto-re-arm | name, interval_ns | yes (next monotonic-epoch-aligned grid line) |
| WaitBarrier (4) | reap + watch N conditions | name, [(watched_name, watched_word, threshold)] | no |

All interlocks are named. No unnamed interlocks. Attach by name only. WaitCounter creation
validates that watched_name exists in the registry; a nonexistent name is rejected. WaitBarrier
validates all watched_names exist.

### Initialization values

| Tier | open_count | closed_count | expiration_ns |
|------|-----------|-------------|---------------|
| Interlock | 0 | 0 | now + 100ms |
| WaitCounter | 0 | 0 | now + 100ms |
| WaitTimer | clock.open_count (now) | clock.open_count (now) | now + 100ms |
| WaitCron | next_grid_line(interval) | clock.open_count (now) | now + 100ms |
| WaitBarrier | 1 | 0 | now + 100ms |

</create-call>

<interlock-shape>

## Interlock shape

Three u64 words in a memfd, 24 bytes:

| Word | Offset | Name | Futex-waitable | Semantics |
|------|--------|------|----------------|-----------|
| 0 | 0 | open_count | yes | monotonic, non-negative, arbitrary increment |
| 1 | 8 | closed_count | yes | monotonic, non-negative, arbitrary increment |
| 2 | 16 | expiration_ns | no | CLOCK_MONOTONIC ns. Future = alive, past or zero = reapable |

Never decremented. open_count and closed_count are independent of expiration_ns.

</interlock-shape>

<field-semantics>

## Field semantics by tier

| Tier | open_count | closed_count | expiration_ns |
|------|-----------|-------------|---------------|
| Interlock | work started (owner r/w) | work completed (owner r/w) | TTL (owner r/w via touch) |
| WaitCounter | target threshold (client r/w) | daemon response: watched value at wake (daemon r/w) | TTL (client r/w, SDK auto-sets 2*W) |
| WaitTimer | target clock time (client r/w) | daemon response: clock value at wake (daemon r/w) | TTL (SDK auto-sets 2*W) |

SDK aliases for WaitCounter/WaitTimer: open_count = `wait_until`, closed_count = `completed_at`.

### Derived properties

| Name | Formula | Type |
|------|---------|------|
| value | open_count - closed_count | signed i64 |
| ttl | expiration_ns - clock_ns | signed i64 |

### States (derived from value and ttl)

| State | Condition | Code |
|-------|-----------|------|
| Closed | value == 0, ttl > 0 | InterlockState::Closed |
| Open | value > 0, ttl > 0 | InterlockState::Open |
| Overrun | value < 0 | InterlockState::Overrun |
| Expired | ttl <= 0 | InterlockState::Expired |

</field-semantics>

<daemon-contract>

## Daemon contract (1 ms best-effort loop)

For every interlock, the daemon derives state and acts:

| State | Action |
|-------|--------|
| Expired (ttl <= 0) | reap (write terminal marker, remove from registry, wake waiters) |
| Overrun (value < 0) | none (informational flag; SDK interprets severity) |
| Open + type WaitCounter | if watched interlock's selected word >= open_count: set closed_count = watched value, futex_wake(closed_count) |
| Open + type WaitTimer | if clock.open_count >= open_count: set closed_count = clock.open_count, futex_wake(closed_count) |
| Open + type WaitCron | if clock.open_count >= open_count: set closed_count = clock.open_count, futex_wake(closed_count), re-arm open_count = next monotonic grid line |
| Open + type WaitBarrier | if all conditions met: set closed_count = clock.open_count, futex_wake(closed_count) |
| Closed, or Open + type Interlock | no action |

The daemon reaps only on expired TTL. Overrun is not daemon-fatal; it is an informational
state the SDK reads and interprets. For wait types, overrun means the daemon was late (the
watched value overshot the target). For bare interlocks, overrun means closed_count exceeded
open_count (an owner bug). In both cases the interlock remains live until its TTL expires.

The clock interlock is advanced by the daemon: open_count = monotonic_now_ms each cycle,
futex_wake(open_count). closed_count is the daemon start time (set once, never changes). The
clock is never reaped; its expiration is refreshed every cycle.

</daemon-contract>

<ttl-rules>

## TTL rules

- Creation TTL is fixed at 100 ms.
- TTL is monotonic-forward (CAS-max): touch sets expiration to max(current, now + duration).
  Never decrements. Touch on a reaped interlock (expiration_ns == 0) returns InterlockReaped.
- WaitTimer wait couples target and TTL: wait_ms(W) sets open_count = clock + W and expiration
  to max(current, now + 2 * W). The 2x margin ensures the interlock cannot be reaped while
  legitimately waiting.

</ttl-rules>

<wake-outcomes>

## Wake outcomes (SDK-side)

| Condition | Test | Meaning |
|-----------|------|---------|
| Normal | closed_count == open_count | woken exactly at target |
| Overrun | closed_count > open_count | woken late; daemon overshot |
| Timeout | open_count > closed_count (futex timed out) | daemon never delivered |

Timeout is fatal for WaitTimer (RTSTimeout, SDK crashes the process). For WaitCounter, timeout
handling is the owner's decision. For bare Interlock, the owner handles timeouts directly.

</wake-outcomes>

<errors>

## Errors

| Error | Source | When | Response |
|-------|--------|------|----------|
| InterlockReaped | daemon | TTL lapsed or name claimed by another create | recreate and resume, or crash |
| InterlockNotFound | daemon | attach to a name not in the registry, or WaitCounter watched_name not found | re-check the name |
| RTSTimeout | SDK | WaitTimer futex timed out at 2*W without daemon wake | fatal, SDK crashes the process |
| RTSOverrun | SDK | closed_count > open_count after wake | non-fatal, surfaces to caller |

</errors>

<permission-model>

## Permission model (SDK-enforced)

| Role | open_count | closed_count | expiration_ns |
|------|-----------|-------------|---------------|
| Creator (Interlock) | r/w | r/w | r/w |
| Creator (WaitCounter/WaitTimer) | r/w | r/o (daemon writes) | r/w |
| Attacher (Interlock) | r/w | r/w | r/o |
| Attacher (WaitCounter) | r/o | r/o | r/o |
| Attacher (clock) | r/o | r/o | r/o |
| WaitTimer | not attachable | -- | -- |

Known limitation: the Attached wire response carries only the id, not the tier. The SDK
cannot enforce tier-specific permissions on attach. Full enforcement requires a wire change
to include the tier in the Attached response.

</permission-model>

<wire-abi>

## Wire ABI

Protocol version 1. Length-prefixed binary frames over UDS with SCM_RIGHTS fd passing.
Version byte leads the payload (before the tag).

| Property | Value |
|----------|-------|
| Frame | u32 LE length prefix + payload |
| Version | u8 (first payload byte, currently 1) |
| Max payload | 4096 bytes |
| String encoding | u16 LE length prefix + UTF-8 bytes |
| Integer encoding | u64 LE |
| Fds per response | 1 (Created, Attached) or 0 (Error) |

Request tags (after version byte):

| Tag | Request | Payload |
|-----|---------|---------|
| 0x01 | Create | name(string) tier(u8) [tier=1: watched_name(string) watched_word(u8)] [tier=3: interval_ns(u64)] [tier=4: condition_count(u16) then for each: watched_name(string) watched_word(u8) threshold(u64)] |
| 0x02 | Attach | name(string) |

Response tags (after version byte):

| Tag | Response | Payload |
|-----|----------|---------|
| 0x81 | Created | id(u64) |
| 0x82 | Attached | id(u64) |
| 0x88 | Error | code(u8) message(string) |

Error codes: 0x01 InterlockReaped, 0x02 InterlockNotFound, 0x03 AllocationFailed, 0x04 InvalidRequest.

Tier discriminants: 0 = Interlock, 1 = WaitCounter, 2 = WaitTimer, 3 = WaitCron, 4 = WaitBarrier.
WatchedWord: 0 = open_count, 1 = closed_count.

</wire-abi>

<daemon-sdk-boundary>

## Daemon-to-SDK boundary

| Concern | Owner |
|---------|-------|
| create and attach over UDS, returning fds | daemon |
| derive state (value, ttl) and act (reap, wake) | daemon |
| the 1 ms loop and the clock interlock | daemon |
| validate watched_name at WaitCounter/WaitBarrier create | daemon |
| WaitCron auto-re-arm to next grid line | daemon |
| WaitBarrier all-conditions check | daemon |
| attach ergonomics, handle lifetime | SDK |
| background touch thread (auto-keepalive) | SDK |
| wait_until / completed_at aliases | SDK |
| futex_wait with timeout, futex_wake for bare interlocks | SDK |
| RTSTimeout fatal crash | SDK |
| RTSOverrun detection and surfacing | SDK |
| permission model enforcement | SDK |
| composition (WaitBarrier, WaitRace, WaitSequence) | SDK |

Rule: the daemon provides one primitive and one state machine. The SDK provides the type
hierarchy, the ergonomics, and the composition.

</daemon-sdk-boundary>

<excluded>

## Explicitly excluded

Not part of Abacus: core pinning, CPU isolation, SCHED_FIFO placement, deployment topology,
the Convoy pod layout, any consumer fail-safe cascade, and any knowledge of who consumes
Abacus. These are deployment and consumer concerns.

</excluded>
