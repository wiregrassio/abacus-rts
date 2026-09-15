<purpose>
# rf-rts/

RF-RTS: the Roboflow Real-Time Scheduler. A coordination primitive daemon. It provides one
primitive, the interlock, and reaps interlocks whose heartbeat has expired. Counters and timers
are contracts layered over the interlock, provided by the client SDK. Runs as a systemd service,
zero dependencies but systemd. Standalone; other systems (Convoy, and more) depend on it.
</purpose>

<contracts>
## Contracts

- `docs/KICKOFF.md` is the LLM entry point. Read it cold before touching anything else.
- Daemon versus SDK: the daemon provides interlocks and reaps them; the SDK provides everything
  that makes an interlock feel like a timer or counter. Consumer-specific behavior is SDK.
- If it is not part of the daemon interface or its contract, it does not belong in this repo.
  Deployment topology, core pinning, and consumer cascades are outside RTS and not recorded here.
- No em or en dashes anywhere. Shortest-complete register.
</contracts>

<files>
## Files

| File | Purpose |
|------|---------|
| `README.md` | Project overview and pointer to the primary design document. |
</files>

<subdirectories>
## Subdirectories

| Directory | Purpose |
|-----------|---------|
| `docs/` | Design documents: the LLM kickoff, the architecture-rung design set, and research. |
</subdirectories>
