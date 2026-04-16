# F10 ISR Unblock Boundary Diagnostic Exit

## What I built

- `kernel/src/drivers/ahci/mod.rs`: added AHCI ring site tags
  `UNBLOCK_ENTRY`, `UNBLOCK_AFTER_CPU`, `UNBLOCK_AFTER_BUFFER`,
  `UNBLOCK_AFTER_NEED_RESCHED`, `UNBLOCK_BEFORE_SGI_SCAN`,
  `UNBLOCK_PER_SGI`, and `UNBLOCK_EXIT`; extended site-name rendering.
- `kernel/src/task/scheduler.rs`: added seven ring-only breadcrumbs inside
  `isr_unblock_for_io(tid)`.
- `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`: appended the F10 sweep,
  verbatim stalled-token extract, last-site verdict, candidate ranking, and
  F11 recommendation.
- `.factory-runs/arm64-f10-isr-unblock-boundary-20260416-050814/scratchpad.md`:
  maintained the run notebook.
- `.factory-runs/arm64-f10-isr-unblock-boundary-20260416-050814/decisions.md`:
  documented the choice to encode per-SGI target CPU in `slot_mask`.

## What the original ask was

Branch from `diagnostic/f9-completion-boundary`, add internal AHCI-ring
breadcrumbs to `scheduler::isr_unblock_for_io(tid)` without changing scheduler
semantics, run a five-run Parallels sweep, and use the new `UNBLOCK_*` sequence
to identify where the stuck wake call stopped.

## How what I built meets that ask

- **Implemented**: branch `diagnostic/f10-isr-unblock-boundary` was created
  from `diagnostic/f9-completion-boundary`.
- **Implemented**: new AHCI site constants and display names were added in
  `kernel/src/drivers/ahci/mod.rs:266` and `kernel/src/drivers/ahci/mod.rs:415`.
- **Implemented**: `isr_unblock_for_io(tid)` breadcrumbs were added in
  `kernel/src/task/scheduler.rs:2529`.
- **Implemented**: `UNBLOCK_PER_SGI` encodes target CPU in `slot_mask`, recorded
  in `decisions.md`.
- **Implemented**: aarch64 build completed with zero warnings/errors.
- **Implemented**: 5x `./run.sh --parallels --test 60` artifacts were captured
  under `logs/breenix-parallels-cpu0/f10-isr-unblock/run{1..5}/`.
- **Implemented**: each `summary.txt` contains all F9 fields plus the seven
  requested `UNBLOCK_*` counts.
- **Implemented**: investigation doc appended at
  `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md:2364`.

## What I did NOT build

- I did not change scheduler, SGI, or wake-buffer semantics.
- I did not change F7 admission, F8 ring design, or F9 completion-boundary code.
- I did not touch Tier-1 or Tier-2 prohibited files.
- I did not add logging macros, locks, heap allocation, or new trace-ring
  fields.

## Known risks and gaps

- All five `run.sh` invocations returned `exit_status=1`, consistent with F9's
  headless screenshot/test helper behavior; serial logs were captured and used
  for diagnosis.
- The F10 sweep exposed two internal signatures: runs 1 and 3 stop after
  `UNBLOCK_PER_SGI`, while run 2 stops after `UNBLOCK_AFTER_CPU`. The primary
  F11 direction is SGI delivery; wake-buffer push remains a secondary candidate.
- Repo-wide `cargo fmt --check` fails on pre-existing unrelated formatting and
  trailing-whitespace issues, so no broad formatter was applied.
- Beads tracking could not be created because `bd create` failed against the
  local Dolt runtime server (`database "breenix" not found on Dolt server at
  127.0.0.1:63842`).

## Sweep summaries

```text
run1:
exit_status=1
ahci_timeouts=14
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=10
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=2
unblock_after_need_resched=2
unblock_before_sgi_scan=2
unblock_per_sgi=6
unblock_exit=2

run2:
exit_status=1
ahci_timeouts=74
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=10
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=12
unblock_exit=2

run3:
exit_status=1
ahci_timeouts=1227
ahci_ring_entries=31
ahci_port0_is=1
ahci_port1_is=1
bsshd_started=1
before_complete=4
after_complete=4
wake_enter=1
wake_exit=1
unblock_entry=1
unblock_after_cpu=1
unblock_after_buffer=1
unblock_after_need_resched=1
unblock_before_sgi_scan=1
unblock_per_sgi=3
unblock_exit=1

run4:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0

run5:
exit_status=1
ahci_timeouts=670
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=6
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=2
unblock_after_need_resched=2
unblock_before_sgi_scan=2
unblock_per_sgi=12
unblock_exit=2
```

## Last-site verdict

Primary sample: `run1`, token `1212`, waiter `tid=11`. The last emitted site is
`UNBLOCK_PER_SGI`, with `slot_mask=0x2` encoding target CPU 2. That event is
emitted immediately before `gic::send_sgi()`, and there is no later
`UNBLOCK_EXIT`, `WAKE_EXIT`, `AFTER_COMPLETE`, or `RETURN` for token `1212`.

Corroborating sample: `run3`, token `1396`, waiter `tid=13`, last emitted site
`UNBLOCK_PER_SGI`, with `slot_mask=0x1` encoding target CPU 1.

Alternate sample: `run2`, token `1263`, waiter `tid=13`, last emitted site
`UNBLOCK_AFTER_CPU`, with no `UNBLOCK_AFTER_BUFFER` for that stuck call.

## Verbatim extract

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   GIC: SPI34 pend=true act=true DAIF=0x300 pend_snap=[0x800004,0x0,0x0]
[ahci]   isr_count=1210 cmd#=1214 completion_done=0 PMR=0xf8 RPR=0xff
[ahci]   port_isr_hits=[1,1209] complete_hits=[0,3]
[ahci]   isr_last_pmr=0xf8 last_port1_IS=0x1 last_port1_cmd_num=1212
[ahci] read_block(522) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=3 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[STUCK_SPI34] AHCI_PORT1_IS=0x1
[AHCI_RING] nsec=2379603791 site=UNBLOCK_PER_SGI cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x2 token=0 wake_success=0 seq=3677
[AHCI_RING] nsec=2379602916 site=UNBLOCK_BEFORE_SGI_SCAN cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3676
[AHCI_RING] nsec=2379602041 site=UNBLOCK_AFTER_NEED_RESCHED cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3675
[AHCI_RING] nsec=2379601166 site=UNBLOCK_AFTER_BUFFER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3674
[AHCI_RING] nsec=2379600291 site=UNBLOCK_AFTER_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3673
[AHCI_RING] nsec=2379599416 site=UNBLOCK_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3672
[AHCI_RING] nsec=2379598541 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1212 wake_success=0 seq=3671
[AHCI_RING] nsec=2379597666 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1212 wake_success=0 seq=3670
[AHCI_RING] nsec=2379596750 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1212 wake_success=0 seq=3669
[AHCI_RING] nsec=2379589250 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1212 wake_success=0 seq=3668
```

## F11 recommendation

F11 should audit SGI delivery first: add ring-only breadcrumbs inside
`gic::send_sgi()` and SGI target construction, before and after each
architectural/system-register write, preserving the target CPU in the event.

Keep a small secondary guard around `IsrWakeupBuffer::push()` so the `run2`
`UNBLOCK_AFTER_CPU` signature can be confirmed or eliminated without changing
scheduler behavior.

## How to verify

```bash
git log --oneline --max-count=2
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f10-aarch64-build.log
grep -E '^(warning|error)' /tmp/f10-aarch64-build.log
git diff --check
for i in 1 2 3 4 5; do cat logs/breenix-parallels-cpu0/f10-isr-unblock/run$i/summary.txt; done
rg -n "site=(UNBLOCK_|WAKE_|BEFORE_COMPLETE|AFTER_COMPLETE|ENTER|POST_CLEAR|RETURN)" logs/breenix-parallels-cpu0/f10-isr-unblock/run1/serial.log | tail -40
```
