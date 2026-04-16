# F9 Completion Boundary Diagnostic Exit

## What I built

- `kernel/src/drivers/ahci/mod.rs`: added AHCI ring site tags `BEFORE_COMPLETE`, `AFTER_COMPLETE`, `WAKE_ENTER`, and `WAKE_EXIT`; made `push_ahci_event()` crate-visible for diagnostic use by `Completion::complete()`.
- `kernel/src/drivers/ahci/mod.rs`: wrapped the existing `AHCI_COMPLETIONS[port][0].complete(cmd_num)` call with `BEFORE_COMPLETE` / `AFTER_COMPLETE` ring pushes.
- `kernel/src/drivers/ahci/mod.rs`: added `port_is_snapshot(port)` while preserving `port0_is_snapshot()`.
- `kernel/src/task/completion.rs`: wrapped the existing `scheduler::isr_unblock_for_io(tid)` call with `WAKE_ENTER` / `WAKE_EXIT` ring pushes.
- `kernel/src/arch_impl/aarch64/gic.rs`: extended SPI34 stuck-state output from only `AHCI_PORT0_IS` to both `AHCI_PORT0_IS` and `AHCI_PORT1_IS`.
- `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`: appended the F9 sweep results, verbatim run 2 extract, Case A verdict, and F10 recommendation.
- `.factory-runs/arm64-f9-completion-boundary-20260416-044804/scratchpad.md`: maintained the factory scratchpad.

## What the original ask was

Add boundary diagnostics around the AHCI handler's `Completion::complete()` call and the scheduler wake helper it invokes, run a five-run Parallels sweep from the F8 diagnostic base, then use the new ring data to decide whether the stall is before completion, inside completion, in scheduler wake, after completion, or earlier in the handler.

## How what I built meets that ask

- **Implemented**: branch `diagnostic/f9-completion-boundary` was created from `diagnostic/f8-ahci-completion`.
- **Implemented**: `BEFORE_COMPLETE` / `AFTER_COMPLETE` site tags were added and emitted around `Completion::complete()` in `kernel/src/drivers/ahci/mod.rs:259` and `kernel/src/drivers/ahci/mod.rs:2670`.
- **Implemented**: `WAKE_ENTER` / `WAKE_EXIT` site tags were added and emitted around `isr_unblock_for_io(tid)` in `kernel/src/task/completion.rs:495`.
- **Implemented**: `push_ahci_event()` was exposed as `pub(crate)` in `kernel/src/drivers/ahci/mod.rs:364`.
- **Implemented**: SPI34 stuck-state dump now emits both `AHCI_PORT0_IS` and `AHCI_PORT1_IS` in `kernel/src/arch_impl/aarch64/gic.rs:997`.
- **Implemented**: 5x `./run.sh --parallels --test 60` sweep artifacts were written under `logs/breenix-parallels-cpu0/f9-completion-boundary/run{1..5}/`.
- **Implemented**: each `summary.txt` includes F8 fields plus `before_complete`, `after_complete`, `wake_enter`, and `wake_exit`.
- **Implemented**: investigation doc appended at `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md:2255`.

## What I did NOT build

- I did not change F7's admission model.
- I did not change F8's existing `ENTER`, `POST_CLEAR`, or `RETURN` sites.
- I did not change `Completion::complete()` semantics; the new code only writes ring events before and after the existing wake call.
- I did not change scheduler wake semantics.
- I did not modify Tier-1 or Tier-2 prohibited files.

## Known risks and gaps

- All five Parallels runs returned `exit_status=1` because the headless screenshot helper could not complete. Serial logs were captured and used for the diagnostic verdict.
- Run 1 hit a soft-lockup branch before `bsshd` and did not produce AHCI ring data.
- Runs 3-5 reached `bsshd` without AHCI timeout/ring output in the 60-second collector window.
- Run 2 is the only AHCI timeout sample in this five-run sweep, but it captured the required stalled-token boundary data.
- Beads tracking could not be updated because `bd create` failed against the local Dolt runtime server (`database "breenix" not found on Dolt server at 127.0.0.1:63842`). `bd bootstrap` synced from remote but did not fix the runtime mismatch.

## Sweep summaries

```text
run1:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=0
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0

run2:
exit_status=1
ahci_timeouts=1
ahci_ring_entries=32
ahci_port0_is=1
ahci_port1_is=1
bsshd_started=1
before_complete=7
after_complete=5
wake_enter=3
wake_exit=2

run3:
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

run5:
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
```

## Case verdict

**Case A by the top-level F9 decision rule.** The stalled token is `1219`: it has `BEFORE_COMPLETE` without `AFTER_COMPLETE`, so the handler is stuck inside `Completion::complete()`.

The new wake-side data narrows that further: token `1219` also has `WAKE_ENTER` without `WAKE_EXIT`, so the specific in-`complete()` stall is inside `scheduler::isr_unblock_for_io(tid)`, not in the initial `done.store`, fence, `sev`, or waiter load.

## Verbatim ring extract

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   GIC: SPI34 pend=true act=true DAIF=0x300 pend_snap=[0x800004,0x0,0x0]
[ahci]   isr_count=1217 cmd#=1221 completion_done=0 PMR=0xf8 RPR=0xff
[ahci]   port_isr_hits=[1,1216] complete_hits=[0,10]
[ahci] read_block(522) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=6 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[STUCK_SPI34] AHCI_PORT1_IS=0x1
[AHCI_RING] nsec=2366781458 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1219 wake_success=0 seq=3690
[AHCI_RING] nsec=2366780583 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1219 wake_success=0 seq=3689
[AHCI_RING] nsec=2366779666 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1219 wake_success=0 seq=3688
[AHCI_RING] nsec=2366771250 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1219 wake_success=0 seq=3687
[AHCI_RING] nsec=2364838458 site=RETURN cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=1 seq=3686
[AHCI_RING] nsec=2364837541 site=AFTER_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=0 seq=3685
[AHCI_RING] nsec=2364836541 site=WAKE_EXIT cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1218 wake_success=0 seq=3684
[AHCI_RING] nsec=2364808208 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1218 wake_success=0 seq=3683
[AHCI_RING] nsec=2364807250 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=0 seq=3682
```

## F10 recommendation

Probe name: `f10-isr-unblock-boundary`.

Instrument `kernel/src/task/scheduler.rs::isr_unblock_for_io(tid)` with additional AHCI ring sites at:

- after `current_cpu_id_raw()`
- after `ISR_WAKEUP_BUFFERS[cpu].push(tid)`
- after `set_need_resched()`
- before the idle-CPU SGI scan loop
- before each `send_sgi()`
- after each `send_sgi()`
- final exit

Keep the probe atomic/ring-only. If the last emitted site is before or inside `send_sgi()`, F10 should audit SGI delivery/GICR targeting. If the last emitted site is before `set_need_resched()` or the wake-buffer push, F10 should audit per-CPU wake buffer atomics.

## How to verify

```bash
git log --oneline --max-count=2
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f9-aarch64-build.log
grep -E '^(warning|error)' /tmp/f9-aarch64-build.log
git diff --check
for i in 1 2 3 4 5; do cat logs/breenix-parallels-cpu0/f9-completion-boundary/run$i/summary.txt; done
rg -n "site=(BEFORE_COMPLETE|AFTER_COMPLETE|WAKE_ENTER|WAKE_EXIT)|site=(ENTER|POST_CLEAR|RETURN)" logs/breenix-parallels-cpu0/f9-completion-boundary/run2/serial.tail.txt | tail -40
```
