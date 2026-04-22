# F34 Scratchpad

## 2026-04-22: start

Contract: investigate CPU0 virtual timer degradation after `schedule_from_kernel`
to `wait_timeout` to idle on Parallels/HVF. Strict phase order requires Phase 1
reproduction before root cause or fix work.

Created and claimed Beads issue `breenix-eru`.

## Phase 1 attempts

Baseline run from `5780377f`:

```bash
pkill -9 qemu-system-x86 2>/dev/null; killall -9 qemu-system-x86_64 2>/dev/null; pgrep -l qemu || echo "All QEMU processes killed"
./run.sh --parallels --clean --test 120
```

Artifact copy: `logs/f34/baseline-main/serial.log` and screenshot.

Result: no reproduction. The boot reached bsshd and bounce. CPU0 timer heartbeat
continued through `cpu0 ticks=90000`; compositor reached frame `23500`; no AHCI
timeout and no trace dump.

Temporary instrumentation added, built clean, then removed before exit because no
degradation trace was captured and carrying unproven hot-path probes forward
would violate the factory scope.

Instrumented test run:

```bash
./run.sh --parallels --test 120
```

Artifact copy: `logs/f34/phase1-instrumented-run1/serial.log` and screenshot.

Result: no reproduction. The boot reached bsshd and bounce. Compositor reached
frame `24000`; no AHCI timeout and no trace dump. This run did not emit the
periodic `cpu0 ticks=` heartbeat lines in the captured serial, but the system
kept rendering and did not show the requested degradation failure.

Live instrumented run:

```bash
./run.sh --parallels --no-build
```

Artifact copy: `logs/f34/live-instrumented-run2/serial.log` and screenshot.

Result: no reproduction. The boot reached bsshd and bounce. CPU0 timer heartbeat
continued through `cpu0 ticks=60000`; compositor reached frame `15000`; no AHCI
timeout and no trace dump.

Host TCP probes to `10.211.55.100:2222`, `:23`, and `:2323` did not connect;
guest serial showed bsshd listening, but the guest IP was not host-reachable
from this session.

## Phase 2 evidence collected

Linux-probe collection command:

```bash
LINUX_PROBE_USER=wrb LINUX_PROBE_PASSWORD=root LINUX_PROBE_SUDO_PASSWORD=root \
  scripts/parallels/collect-linux-cpu0-traces.sh logs/f34/linux-probe-20260422-044652
```

Root SSH was rejected; the repository script's default `wrb` login with password
`root` worked.

Linux reference artifacts:

- `logs/f34/linux-probe-20260422-044652/env.txt`
- `logs/f34/linux-probe-20260422-044652/cpu0-events.report.txt`
- `logs/f34/linux-probe-20260422-044652/cpu0-fg.report.txt`
- `logs/f34/linux-probe-20260422-044652/interrupts.before.txt`
- `logs/f34/linux-probe-20260422-044652/interrupts.after.txt`

Findings are summarized in `linux-probe-comparison.md`.

## Stop decision

Phase 1 did not reproduce the target failure after three autonomous Breenix
boots. The supplied trace may be valid for another local state, but current
`5780377f` in this workspace does not show it under the requested run path.

Stopped before root-cause and fix work. No Linux-parity fix was implemented
because there is no current Breenix failure trace to compare against Linux.
