# F17 Exit Report: Local IRQ-Return need_resched + Wake-Buffer Drain Audit

## Status

VERDICT: FAIL.

F17 disproved the two scheduler-local hypotheses and did not apply a scheduler fix. The remaining failure is outside the local wake-buffer / IRQ-return resched path.

Branch: `diagnostic-fix/f17-local-wake`
Base: `4bd74caa probe/f16-idle-scan-fix`
Primary commit: `diagnostic(arm64): local wake + resched tail breadcrumbs`
Result commit: `investigation(arm64): F17 local wake audit sweep results`

## Build

AArch64 kernel build completed cleanly with no warning/error lines:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Final build log: `logs/breenix-parallels-cpu0/f17-local-wake/build-final.log`

## Validation

Command run five times:

```bash
./run.sh --parallels --test 60
```

Logs:

```text
logs/breenix-parallels-cpu0/f17-local-wake/run1/
logs/breenix-parallels-cpu0/f17-local-wake/run2/
logs/breenix-parallels-cpu0/f17-local-wake/run3/
logs/breenix-parallels-cpu0/f17-local-wake/run4/
logs/breenix-parallels-cpu0/f17-local-wake/run5/
```

## Sweep Summary

| run | bsshd_started | ahci_timeouts | corruption_markers | failed_exec | soft_lockups | scan_start | scan_cpu | scan_done | unblock_before_send_sgi | unblock_after_send_sgi | ttwu_local_entry | ttwu_local_set_resched | irq_tail_check_resched | resched_check_entry | resched_check_drained_wake | resched_check_switched | resched_check_return | result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| run1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | PASS |
| run2 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | PASS |
| run3 | 1 | 2 | 0 | 1 | 2 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 3 | 3 | 3 | 1 | 2 | FAIL |
| run4 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | PASS |
| run5 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | PASS |

Pass criteria were not met because run3 had two AHCI timeouts after reaching `[init] bsshd started (PID 2)`.

## H1/H2 Determination

H1: falsified. The IRQ-return scheduler check is reached. The preserved sequence includes `IRQ_TAIL_CHECK_RESCHED` followed by `RESCHED_CHECK_ENTRY`.

H2: falsified. The wake buffer is drained before the scheduling decision. The preserved sequence includes `RESCHED_CHECK_DRAINED_WAKE wake_success=1` before `RESCHED_CHECK_SWITCHED wake_success=1`.

Confirmed bucket: Other.

## Key Evidence

Diagnostic-run2 preserved the full expected sequence including `TTWU_LOCAL_SET_RESCHED`:

```text
TTWU_LOCAL_ENTRY cpu=0 waiter_tid=11 slot_mask=0x0 token=1
WAKEBUF_AFTER_PUSH waiter_tid=11
TTWU_LOCAL_SET_RESCHED cpu=0 waiter_tid=11 slot_mask=0x1 token=1 wake_success=1
IRQ_TAIL_CHECK_RESCHED cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_ENTRY cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_DRAINED_WAKE cpu=0 waiter_tid=1 slot_mask=0x1 token=1 wake_success=1
RESCHED_CHECK_SWITCHED cpu=0 waiter_tid=11 token=1 wake_success=1
```

Final run3 preserved the same scheduler decision path, with the local-set marker slot missing from the dump but the IRQ-tail marker confirming `need_resched=1` and wake-buffer depth `1`:

```text
TTWU_LOCAL_ENTRY cpu=0 waiter_tid=11 slot_mask=0x0 token=1
WAKEBUF_AFTER_PUSH waiter_tid=11
IRQ_TAIL_CHECK_RESCHED cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_ENTRY cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_DRAINED_WAKE cpu=0 waiter_tid=1 slot_mask=0x1 token=1 wake_success=1
RESCHED_CHECK_SWITCHED cpu=0 waiter_tid=11 token=1 wake_success=1
RESCHED_CHECK_RETURN cpu=0 slot_mask=0x0 token=0
```

Run3 then timed out on a later AHCI command, with the stuck-state dump reporting:

```text
GICD_ISPENDR[1]=0x800004 bit=2 pending=true
GICD_ISACTIVER[1]=0x4 bit=2 active=true
AHCI_PORT1_IS=0x1
CI=0x0
cmd#=1374
last_port1_cmd_num=1372
```

This shows the local wake completed and switched before the later timeout. The last local-wake breadcrumb is `RESCHED_CHECK_SWITCHED`; the last F17 breadcrumb visible in the timeout dump is `RESCHED_CHECK_RETURN`.

## Fix Description

No scheduler-path fix was applied. F17 added diagnostic breadcrumbs only. A local wake-buffer drain or missing IRQ-tail resched call would be the wrong fix for the observed evidence.

## Next Step Recommendation

Open F18 against the AHCI/SPI34 active-pending corridor. Investigate why SPI34 remains active/pending with AHCI Port1 `IS=0x1` and no completion for a later command after a successful local wake. Likely audit areas are AHCI interrupt completion / EOI-deactivation ordering and AHCI command publication/completion state, not the local IRQ-return scheduler drain path.
