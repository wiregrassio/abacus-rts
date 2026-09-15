# rf-RTOS: kickoff

Standalone design doc. Read cold, build from it. A soft-real-time liveness scheduler for
containers on Jetson — the RTOS-equivalent coordinator that replaces the current 1-second
heartbeat poll. Not motion control, not hard-real-time, not safety-rated.

Sibling context (optional): `research-rf-rtos.md` (Linux RT facilities deep-dive),
`brief-alex-norell-roboflow-os.md` (RF OS / deployment-target context).

---

## Thesis + contract

One busy-loop increments a shared-memory atomic tick counter on an absolute-time schedule.
Containers `futex_wait` on child counters derived from it. That's the whole system: waitable
counters + one honest clock.

- **Contract:** 10 ms tick, 1σ ≈ ±1 ms (±2 ms acceptable; stated claim is ±1 ms).
- **Referent:** FreeRTOS-on-ESP32 default tick is 1 ms / 1000 Hz. We're matching an RTOS
  reference tick in userspace on a 12-core A78AE. This is not ambitious.
- **Baseline it beats:** current liveness is a 1 s heartbeat. Even a 100 ms tick is 10×
  better. We sit ~2 orders of magnitude inside "good enough for production."
- **Bar it clears:** tighter than an Allen-Bradley ControlLogix scan-jitter spec. This is a
  PLC-class problem, not an NSA-timing one. We're checking Docker containers are alive.
- **Load reality:** ~3–4 tasks executed per tick, work-per-tick sub-1 ms, everything
  downstream tolerates ≥100 ms. Never a saturated system.

The whole hard-real-time rigor in `research-rf-rtos.md` (L4 cache, MPAM, MOESI,
cross-cluster coherence, 100 µs loops, PREEMPT_RT) is **v2 optimization surface, not needed
now.** Do not lead with it.

---

## v0 — the version that ships (do nothing exotic)

A userspace daemon. Rust or Python. No pinning, no SCHED_FIFO, no isolcpus, no kernel
rebuild required to hold the contract.

```
loop {
    next += 10ms;
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, next);   // absolute, not relative
    tick_global.fetch_add(1, Release);                        // shared-memory atomic
    futex_wake(&tick_global, ALL);                            // waiters re-read, compare, act
}
```

**The one non-negotiable: absolute deadlines** (`next += 10ms` against a running anchor,
`TIMER_ABSTIME`). Relative `nanosleep` accumulates wakeup latency every cycle and drifts out
of contract within a minute. Absolute re-anchors each tick to the monotonic clock, so jitter
stays bounded per-tick instead of summing. Everything else is optional tuning.

**Why "do nothing" works:** CFS gives a runnable task its slice within a quantum (single-digit
ms). To blow ±1 ms systematically you'd need all cores saturated with equal-or-higher-priority
work across consecutive 10 ms windows — which 3–4 liveness checks/tick never produce. At worst
synthetic full-machine load (`stress-ng --cpu 12`) it degrades to maybe ±5 ms, and at that load
the box is already on fire and missing heartbeats for real reasons. Confidence the bare daemon
holds 10 ms ±1 ms 1σ under normal load: ~90%.

**Packaging:** core is just waitable counters + absolute-time loop → ships as a standalone
daemon with **no Convoy dependency.** Convoy integration is an optional deployment mode.
Viable as a standalone open-source release. ~6 months out; not a current deliverable.

---

## Tick vocabulary (child counters)

Same parent/child convention as Convoy's coordinate system. `tick_global` is the monotonic
counter the singleton increments; channels are derived by mod.

| Variable | Meaning |
|----------|---------|
| `tick_global` | Monotonic tick counter (the RTOS singleton increments this) |
| `tick_10ms` | 10 ms channel: `tick_global mod (10ms / tick_interval)` |
| `tick_25ms` | 25 ms channel |
| `tick_50ms` | 50 ms channel |
| `tick_100ms` | 100 ms health-check channel |

A process subscribes to a channel by waiting on the appropriate child counter. Timer =
interlock sugar: begin = "going to sleep", end = "wake up" (the RTOS fires it). The whole
scheduling system is one primitive.

**JWatch integration:** nanosecond timestamps on wake/sleep + CPU core ID as metadata.
Measures exact jitter — this is how the ±1 ms claim gets verified rather than asserted.

---

## Escalation ladder (strictly optional, measurement-gated, LATER)

Each rung buys tail-latency margin v0 doesn't need. Add only if JWatch shows the contract
slipping — not as build steps.

1. **SCHED_FIFO** — `cap_add: SYS_NICE` + `ulimits.rtprio` in compose, checked
   `sched_setscheduler` (fail loud on EPERM, sentinel — don't run silently unpinned).
2. **cpuset pin** — Docker `cpuset_cpus`, or self-pin via `sched_setaffinity` in-process.
3. **isolcpus boot param** — evict others from the core (below).
4. **PREEMPT_RT / nohz_full** — µs-shaving. Never needed for a liveness tick.

---

## Full isolated-core architecture (the target layout, when we want it)

This is where the design goes when it graduates from "background daemon" to "dedicated
cores." Everything below is the measurement-gated target, not v0.

### Core map — low-numbered fixed roles, workers grow upward

Portable across SKUs: cores 0–5 are infrastructure, everything ≥6 is a worker slot.
**Worker slots = `nproc − 6`.** 12-core AGX → 6 workers (6–11). 8-core part → 2 workers (6–7).
Generate the isolcpus range from `nproc` at provision time; don't hand-anchor the end.

| Cores | Role | Treatment |
|-------|------|-----------|
| 0 | IRQ sink + boot-required only | best-effort kept clear (not isolated; isolating CPU0 is disallowed) |
| 1–3 | Housekeeping / scheduler | OS, Docker, RCU offload threads, background containers |
| 4 | RTOS daemon | isolated + tickless + RCU-off, SCHED_FIFO ~80 on its own core |
| 5 | NIC IRQ core | isolated; falls back to core 0 if IRQ unreroutable |
| 6…N−1 | Convoy camera-worker pods (one per core, up to 6 cameras) | isolated; internal FIFO ladder |

### extlinux boot line

File: `/boot/extlinux/extlinux.conf`. Append to the `APPEND ${cbootargs}...` line under the
primary `LABEL`. **Back up first** (`cp extlinux.conf extlinux.conf.bak`) — a malformed
APPEND can fail to boot, and that's a bad day on a field Jetson with no SSH. Reboot required.

12-core:
```
isolcpus=4-11 nohz_full=4,5 rcu_nocbs=4-11 rcu_nocb_poll irqaffinity=1-3
```
8-core:
```
isolcpus=4-7 nohz_full=4,5 rcu_nocbs=4-7 rcu_nocb_poll irqaffinity=1-3
```

Per-token:
- `isolcpus=4-N` — pull all worker + infra cores off the load balancer. Nothing lands unless
  explicitly placed. Compiled into stock L4T; works with no kernel rebuild.
- `nohz_full=4,5` — tickless on the two determinism-sensitive cores (RTOS + IRQ) only.
  Best-effort: **silently ignored if stock kernel lacks `CONFIG_NO_HZ_FULL`** — harmless,
  that's fine, we're not depending on it.
- `rcu_nocbs=4-N` — offload RCU callbacks off all isolated cores (see below).
- `rcu_nocb_poll` — poll-mode offload, drops the wakeup IPIs.
- `irqaffinity=1-3` — default all IRQs to housekeeping cores 1–3, leaving core 0 clear as
  the sink for whatever's hard-wired to it.

Verify after boot: `cat /sys/devices/system/cpu/isolated` → `4-11` (or `4-7`).

### RCU callbacks — what `rcu_nocbs` does

RCU (read-copy-update) defers freeing protected memory to a "callback" that normally runs in
softirq context **on the same core that queued it**. On an isolated core that's uninvited work
that steals cycles and re-arms the tick. `rcu_nocbs=<cores>` offloads those callbacks to
dedicated `rcuo` kthreads running on the **non-isolated housekeeping cores** instead. The
isolated core queues and moves on; core 1–3 does the cleanup. This is the mechanism that makes
`isolcpus` actually quiet rather than nominally quiet. `rcu_nocb_poll` makes the offload threads
poll rather than wake by IPI — one fewer interrupt on the isolated side.

### NIC IRQ pinning — best-effort, can't lose

Try to move the NIC IRQ onto core 5 (or 0). If Tegra wiring won't allow it, it stays on 0,
which is already reserved for exactly that.

```bash
grep eth0 /proc/interrupts                              # left column = IRQ number(s)
echo 5 | sudo tee /proc/irq/<IRQ>/smp_affinity_list     # try to place it
cat /proc/irq/<IRQ>/effective_affinity_list             # PROOF it moved — must read 5, not 0
```

**Tegra gotcha:** many Jetson IRQs are hard-wired to CPU0 and cannot be reaffined — the kernel
accepts the write, silently ignores it, routes back to a core that can service it. Always check
`effective_affinity_list`, not just that the write succeeded. If it won't leave 0, that's
hardware, not config — and core 0 is the reserved sink, so the design holds either way. Multi-
queue NICs have one IRQ per queue; knock to single queue with `ethtool -L eth0 combined 1` if
you want one IRQ to place. `/proc/irq/*` resets on reboot → make persistent with a systemd unit
or rc.local; `irqaffinity=1-3` sets the boot default, the runtime write is the exception that
needs re-applying each boot.

Runtime proof the isolation held (sample twice, seconds apart): `cat /proc/interrupts` — core 0
takes the NIC deltas, isolated cores show near-zero IRQ growth.

---

## Worker-pod scheduling — SCHED_FIFO ladder, NOT nice

Each worker core runs one Convoy pod = four operations: **capture, inference, save, Convoy
daemon.** These get strict RT priorities, not `nice`.

| Process | Policy / prio | Rationale |
|---------|---------------|-----------|
| capture | FIFO 90 | 3 ms of work per 30 ms cycle, then blocks on next frame. Hard-preempts everything to grab the frame instantly, then self-suspends. |
| inference | FIFO 80 | Runs every spare cycle while capture sleeps. Yields on every GPU sync (H2D / kernel / D2H) — those stalls are free yield points. |
| Convoy daemon | FIFO 75 | Woken every 10 ms tick, runs sub-ms, has a *timing* obligation (TTL checks / liveness) so it must run on-time. Above save because a wedged daemon that misses its save trigger is worse than a slow save. |
| save | FIFO 70 | Fills inference's GPU-wait gaps interstitially. Slipping a few ms is harmless — frame's already safe in memory. Finishes after the inference result lands. |

RTOS daemon on core 4 is FIFO ~80 on its *own* isolated core, unrelated to this ladder.

### Why RT and not nice — the reasoning that flipped it

Initial instinct was `nice` (capture −19 / inference 0 / save 19) because FIFO can starve.
**It can't here, because the tasks are duty-cycled, not continuous.** Capture is 3 ms/30 ms
then blocks; a blocked FIFO task yields immediately to the next-highest runnable task. So
capture-at-90 means "stop everything, grab the frame, get out of the way" — exactly the
semantics wanted. `nice=-19` capture would instead let inference steal cycles during the 3 ms
acquisition window, adding jitter to the one thing that must be crisp. RT gives the hard
preempt; nice doesn't.

Inference yields naturally: it's CPU-then-wait-then-CPU around GPU round-trips. Every H2D copy,
kernel launch, D2H copy blocks on CUDA sync → the FIFO inference thread yields → save (FIFO 70)
runs in the gap → preempted the instant inference is runnable again. Interstitial save-during-
inference-wait for free, no interleaving logic. The GPU stalls *are* the yield points.

The dependency chain (no save without inference, no inference without capture) is enforced by
**data flow**, not just priority: inference blocks waiting on capture's frame, save blocks
waiting on inference's result. Priority ladder and dependency graph agree, so a lower task can't
starve a higher one it feeds, and the higher task always self-suspends.

### The two hygiene items (the only failure modes left in an all-FIFO pod)

1. **Priority-inherit any cross-level mutex.** Classical unbounded priority inversion needs a
   shared lock held by a low-prio task blocking a high one. The chain doesn't have that shape
   (capture waits on hardware, not a lock save holds) — but if any mutex is shared across levels
   (shared allocator, producer/consumer queue lock), set `PTHREAD_PRIO_INHERIT` on it. Bounded
   inversion is a known, accepted RTOS condition; PI closes the unbounded case.
2. **Keep all FIFO priorities ≤ 90**, below the kernel migration/watchdog threads at 99. A
   userspace task at 99 can wedge the core against kernel maintenance. Already satisfied; don't
   drift up.

---

## Deployment target notes

Current Vantive stack: **Docker Compose on stock JetPack 6.2 + L4T** — no MicroK8s, no RF OS.
So v0 container mechanics are Compose-native (`cpuset_cpus`, `cap_add: SYS_NICE`,
`ulimits.rtprio`), not kubelet CPU-manager. Kubernetes is a planned move; when it lands, the
CPU-manager static-policy path from `research-rf-rtos.md` §3 applies. If the target ever becomes
RF OS (Yocto over L4T), the kernel-config availability of nohz_full/isolcpus/PREEMPT_RT reopens
separately — see the Alex/RF-OS brief. For the current box, all of the above works as written.

Whole Vantive stack uncontained/unisolated at full speed uses **4–5 cores total** (inference 3,
background 1–2). Under the isolated map that fits in housekeeping (1–3) with room; the isolated
side stays pristine. No contention concern at current load.

---

## Start here (next session)

1. Write the v0 daemon (Rust): self-pin optional, absolute-time `clock_nanosleep` loop,
   `tick_global` atomic in `/dev/shm`, `futex_wake` on increment, child-counter channels.
   Fail loud on any setup syscall error (sentinel, greppable).
2. Wire JWatch: ns timestamps on wake/sleep + core ID → measure actual jitter vs the ±1 ms
   claim under real capture+inference load.
3. Ship v0 as-is if the histogram holds. Only then, if a tail shows up, walk the escalation
   ladder — SCHED_FIFO first, isolcpus only if a neighbor is measurably stealing time.
4. The isolated-core architecture + FIFO worker ladder above is the graduation target when
   rf-RTOS moves from standalone daemon to the Convoy per-core deployment.
