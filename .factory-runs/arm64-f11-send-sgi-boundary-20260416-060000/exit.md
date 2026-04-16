# F11 Send SGI Boundary Diagnostic Exit

## What I built

- `kernel/src/drivers/ahci/mod.rs`: added nine AHCI ring site tags for
  `SGI_*` and `WAKEBUF_*`, plus site-name rendering.
- `kernel/src/arch_impl/aarch64/gic.rs`: added seven breadcrumbs inside
  `send_sgi(sgi_id, target_cpu)` and widened the SPI34 postmortem AHCI-ring
  dump to 64 entries for correlation.
- `kernel/src/task/scheduler.rs`: added `WAKEBUF_BEFORE_PUSH` and
  `WAKEBUF_AFTER_PUSH` around the existing `IsrWakeupBuffer::push()` call.
- `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`: appended the F11 sweep,
  summaries, verbatim extracts, last-site verdicts, caveat, and F12
  recommendation.
- `.factory-runs/arm64-f11-send-sgi-boundary-20260416-060000/`: maintained
  prompt, plan, decisions, scratchpad, and this exit document.

## What the original ask was

Start from `diagnostic/f10-isr-unblock-boundary`, create
`diagnostic/f11-send-sgi-boundary`, add AHCI-ring-only breadcrumbs inside the
ARM64 SGI delivery corridor and around wake-buffer push, run five Parallels
samples, and document whether the primary stall stops before/inside/after the
`ICC_SGI1R_EL1` write or inside `IsrWakeupBuffer::push()`.

## How what I built meets that ask

- **Implemented**: branch `diagnostic/f11-send-sgi-boundary` was created from
  `diagnostic/f10-isr-unblock-boundary`.
- **Implemented**: new AHCI site constants were added at
  `kernel/src/drivers/ahci/mod.rs:273`; renderer names at
  `kernel/src/drivers/ahci/mod.rs:431`.
- **Implemented**: seven `send_sgi()` breadcrumbs were added in order at
  `kernel/src/arch_impl/aarch64/gic.rs:1024`.
- **Implemented**: `target_cpu` is encoded in `slot_mask` by
  `trace_sgi_boundary()` at `kernel/src/arch_impl/aarch64/gic.rs:1092`.
- **Implemented**: wake-buffer push breadcrumbs were added around the existing
  call at `kernel/src/task/scheduler.rs:2558`.
- **Implemented**: aarch64 build completed with zero warning/error lines.
- **Implemented**: final five-run sweep captured under
  `logs/breenix-parallels-cpu0/f11-send-sgi/run{1..5}/`.
- **Implemented**: each `summary.txt` contains all F10 fields plus:
  `sgi_entry`, `sgi_after_mpidr`, `sgi_after_compose`, `sgi_before_msr`,
  `sgi_after_msr`, `sgi_after_isb`, `sgi_exit`, `wakebuf_before_push`, and
  `wakebuf_after_push`.
- **Implemented**: investigation section appended at
  `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md:2559`.

## What I did NOT build

- I did not change SGI delivery semantics.
- I did not change `IsrWakeupBuffer::push()` semantics.
- I did not modify F7 admission, F8 ring fields, F9 completion breadcrumbs, or
  F10 unblock breadcrumbs.
- I did not touch prohibited files.
- I did not add logging, locks, heap allocation, or new trace-ring fields inside
  `send_sgi()`.

## Known risks and gaps

- F11 uses existing ring fields only, so SGI calls are correlated by adjacency
  and `slot_mask`, not by a per-call ID.
- The decisive run5 sample shows the stuck wake reaching `SGI_BEFORE_MSR` and
  no `SGI_AFTER_MSR` for that contiguous call. Later same-CPU SGI events exist,
  so F12 should add a per-SGI call ID if it needs absolute single-call proof.
- Runs 1, 2, and 4 did not capture AHCI timeout dumps during the 60-second
  window. Runs 3 and 5 did.
- All `run.sh --parallels --test 60` invocations returned status 1, consistent
  with the existing screenshot/test-helper behavior; serial logs were captured.

## Sweep summaries

```text
run1: exit_status=1 ahci_timeouts=0 ahci_ring_entries=0 sgi_before_msr=0 wakebuf_before_push=0 wakebuf_after_push=0
run2: exit_status=1 ahci_timeouts=0 ahci_ring_entries=0 sgi_before_msr=0 wakebuf_before_push=0 wakebuf_after_push=0
run3: exit_status=1 ahci_timeouts=4 ahci_ring_entries=130 unblock_per_sgi=2 sgi_before_msr=18 sgi_after_msr=18 sgi_after_isb=18 sgi_exit=19
run4: exit_status=1 ahci_timeouts=0 ahci_ring_entries=0 sgi_before_msr=0 wakebuf_before_push=0 wakebuf_after_push=0
run5: exit_status=1 ahci_timeouts=4 ahci_ring_entries=138 unblock_per_sgi=1 sgi_before_msr=17 sgi_after_msr=16 sgi_after_isb=16 sgi_exit=17 wakebuf_before_push=1 wakebuf_after_push=1
```

## Last-Site Verdict

Primary signature: run5, token `1239`, waiter `tid=11`. The stuck wake reached
`UNBLOCK_PER_SGI` with `slot_mask=0x1` (target CPU 1). The contiguous SGI
breadcrumb sequence reached `SGI_BEFORE_MSR`; no `SGI_AFTER_MSR` appeared for
that contiguous call before the trace moved on. Best current verdict:
`ICC_SGI1R_EL1` write corridor.

Secondary signature: the wake-buffer push stall did not reproduce in the
decisive sample. `WAKEBUF_BEFORE_PUSH` and `WAKEBUF_AFTER_PUSH` both appeared
for `tid=11`, so `IsrWakeupBuffer::push()` is lower priority for F12.

## Verbatim Extracts

Primary run5 extract:

```text
[AHCI_RING] nsec=2450015250 site=SGI_BEFORE_MSR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6170
[AHCI_RING] nsec=2450014333 site=SGI_AFTER_COMPOSE cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6169
[AHCI_RING] nsec=2450013291 site=SGI_AFTER_MPIDR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6168
[AHCI_RING] nsec=2450012375 site=SGI_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6167
[AHCI_RING] nsec=2449989500 site=SGI_BEFORE_MSR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6165
[AHCI_RING] nsec=2449988625 site=SGI_AFTER_COMPOSE cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6164
[AHCI_RING] nsec=2449987750 site=SGI_AFTER_MPIDR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6163
[AHCI_RING] nsec=2449986791 site=SGI_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6162
[AHCI_RING] nsec=2449985791 site=UNBLOCK_PER_SGI cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=0 wake_success=0 seq=6161
```

Secondary run5 extract:

```text
[AHCI_RING] nsec=2449983083 site=UNBLOCK_AFTER_BUFFER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6158
[AHCI_RING] nsec=2449982166 site=WAKEBUF_AFTER_PUSH cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6157
[AHCI_RING] nsec=2449981250 site=WAKEBUF_BEFORE_PUSH cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6156
[AHCI_RING] nsec=2449980000 site=UNBLOCK_AFTER_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6155
```

## F12 Recommendation

F12 should investigate the GICv3 system-register SGI write path:
`SGI_BEFORE_MSR` stops means the `msr ICC_SGI1R_EL1, xN` write or its immediate
architectural side effects are now the primary suspect. Verify SRE/ICC state on
the sender and target CPUs and add a per-SGI diagnostic call ID if F12 needs to
remove the remaining correlation caveat.

## How to verify

```bash
git log --oneline --max-count=2
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f11-aarch64-build.log
grep -E '^(warning|error)' /tmp/f11-aarch64-build.log
git diff --check
for i in 1 2 3 4 5; do cat logs/breenix-parallels-cpu0/f11-send-sgi/run$i/summary.txt; done
rg -n 'seq=61(5[0-9]|6[0-9]|7[0-9])|site=(UNBLOCK_|WAKEBUF_|SGI_|WAKE_|BEFORE_COMPLETE|POST_CLEAR|ENTER)' logs/breenix-parallels-cpu0/f11-send-sgi/run5/serial.log
```
