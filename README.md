# Abacus RTS

Lockless atomic coordination for real-time compute. A coordination primitive daemon for
real-time processes on GPU edge devices and anything else that needs sub-millisecond
coordination.

Status: v0 daemon implemented. SDK and test container pending.

Abacus provides one primitive, the interlock: three u64 words (begin, end, heartbeat), two of
them futex-waitable, all monotonic and non-negative. The daemon runs a 1 ms best-effort loop
that reaps interlocks whose heartbeat has expired and wakes waiters whose watched counter has
crossed their target. Counters and timers are contracts layered over the interlock by the client
SDK; they are not separate primitives.

Runs as a systemd service with zero dependencies but systemd. Small enough to fit in cache and
restart in milliseconds. It holds no durable state: a restart comes back empty, wakes no one, and
every waiter discovers the restart through its own futex timeout. Crash-only by design.

The interlock primitive is designed to be depended on by other systems needing shared
coordination: a consumer can become a shared-memory data mover while Abacus owns the interlock.

## Reading order

1. `docs/KICKOFF.md` is the LLM entry point. Read it cold.
2. `docs/design/` is the architecture rung: ARCHITECTURE for the model, CONTRACTS for the frozen
   interface, state machines for the transitions.
3. `docs/research/` grounds the numbers. Two kickoff prompts awaiting a research pass.

## What it is, in one line

One primitive (the interlock), three contracts (interlock, counter, timer), a 1 ms reap-and-wake
loop, handed out over UDS as file descriptors.
