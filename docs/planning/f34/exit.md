# F34 Exit

## What I Built

- `docs/planning/f34/scratchpad.md`: session notebook with commands, artifacts,
  and stop decision.
- `docs/planning/f34/phase1-evidence.md`: Phase 1 reproduction evidence showing
  the requested degradation did not reproduce in three autonomous boots.
- `docs/planning/f34/linux-probe-comparison.md`: Linux-probe runtime/source
  comparison for CPU0 idle, arch_timer, timer programming, and context switch
  patterns.
- `docs/planning/f34/exit.md`: this exit report.

No kernel code is changed in the final commit.

## What The Original Ask Was

Investigate CPU0 virtual timer degradation after the first
`schedule_from_kernel -> wait_timeout -> idle` transition on Parallels/HVF,
prove the failure with trace-framework evidence, compare against Linux on the
same hypervisor, identify the Breenix-vs-Linux divergence, implement a
Linux-parity fix, validate it with repeated 120s Parallels boots, and merge.

## How This Meets The Ask

- Phase 1 reproduction plus instrumentation: **partial**.
  - I ran three autonomous Breenix Parallels boots from `5780377f`.
  - Evidence is recorded in `phase1-evidence.md`.
  - Temporary trace-framework instrumentation was built clean but removed before
    exit because the target failure did not occur and no trace dump was captured.
- Phase 2 Linux-probe comparison: **partial**.
  - Runtime and source findings are recorded in `linux-probe-comparison.md`.
  - Linux-probe collection succeeded with `wrb@10.211.55.3`, password `root`.
- Phase 3 root cause: **not implemented**.
  - Blocked by Phase 1 non-reproduction; there is no current Breenix trace to
    compare against Linux.
- Phase 4 fix: **not implemented**.
  - A fix without reproduced failure evidence would be a workaround or guess,
    which the prompt forbids.
- Phase 5-6 validation sweeps: **not implemented**.
  - No fix exists to validate.
- Phase 7 PR and merge: **not implemented**.
  - No fix branch was produced.

## What I Did Not Build

- I did not commit trace instrumentation.
- I did not write `root-cause.md`; the available evidence does not support a
  specific root cause.
- I did not modify the ret-based idle dispatch, timer programming, or
  `wait_timeout` path.
- I did not open or merge a PR.

## Known Risks And Gaps

- The user-supplied trace may have come from a different local tree state or a
  stochastic failure that did not appear in these runs.
- Host TCP access to the Breenix guest did not work from this session, so I
  could not read `/proc/trace/buffer` from a successful boot.
- Linux-probe shows tickless/nohz behavior rather than a continuous 1ms timer
  cadence in idle. It is still useful evidence that arch_timer interrupts
  continue across idle transitions on Parallels/HVF.
- The ignored `logs/f34/*` artifacts are local only and are not committed.

## How To Verify

Inspect the committed docs:

```bash
sed -n '1,220p' docs/planning/f34/phase1-evidence.md
sed -n '1,260p' docs/planning/f34/linux-probe-comparison.md
```

Re-run the Breenix reproduction attempt:

```bash
pkill -9 qemu-system-x86 2>/dev/null; killall -9 qemu-system-x86_64 2>/dev/null; pgrep -l qemu || echo "All QEMU processes killed"
./run.sh --parallels --clean --test 120
```

Re-run the Linux probe:

```bash
LINUX_PROBE_USER=wrb LINUX_PROBE_PASSWORD=root LINUX_PROBE_SUDO_PASSWORD=root \
  scripts/parallels/collect-linux-cpu0-traces.sh logs/f34/linux-probe-$(date +%Y%m%d-%H%M%S)
```

Quality gates run before exit:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

cargo build --release --features testing,external_test_bins --bin qemu-uefi
```
