# F32i CPU0 WFI Wake Diagnosis

Date: 2026-04-19

Scope: diagnosis only. The temporary probes used for this document were reverted before committing this file. No Tier 1 files were modified for the final commit.

## Instrumentation

Temporary lock-free trace points were added around the aarch64 idle WFI path, reschedule IPI send/receive, and CPU0 runqueue enqueue. The probes emitted to the existing in-memory trace buffers and were dumped only after a CPU1 watchdog detected CPU0 timer progress had stopped. No serial breadcrumbs were added in the hot path.

One deliberate exception: the probe did not read `ICC_IAR1_EL1` immediately before WFI. Reading IAR acknowledges the pending interrupt and would change the bug being measured. Instead, the real SGI receive path recorded IAR-derived interrupt IDs; no `F32I_IPI_RECV` events were present in either hang dump.

Artifacts:

- Run 4: `.factory-runs/f32i-cpu0-wfi-wake-20260419/parallels-run4/serial.log`
- Run 5: `.factory-runs/f32i-cpu0-wfi-wake-20260419/parallels-run5/serial.log`

## Reproduction Summary

Two Parallels hangs reproduced the same CPU0 signature:

| Run | Watchdog | CPU0 state | Runnable init | Timer state |
| --- | --- | --- | --- | --- |
| Run 4 | `CPU1 observed CPU0 timer stuck at 77 while CPU1 reached 2000` at line 392 | `CPU 0: current=0 previous=10` at line 402 | `tid=10 state=R user pid=1` at line 421 | global ticks stuck at 77 while total timer IRQ count reached 21350 |
| Run 5 | `CPU1 observed CPU0 timer stuck at 137 while CPU1 reached 2000` at line 380 | `CPU 0: current=0 previous=10` at line 390 | `tid=10 state=R bis user pid=1` at line 409 | global ticks stuck at 137 while total timer IRQ count reached 19735 |

## Evidence Answers

1. Was an IPI sent to CPU0 after `wake_io_thread_locked` enqueued tid 10?

Yes. Run 4 preserved the enqueue and send:

- Line 2371: `F32I_RQ_ENQUEUE` from CPU1, payload `65546` = target CPU0, queue length 1, tid 10. Flags show the target CPU need bit was set.
- Line 2372: `F32I_IPI_SEND` from CPU1, payload `16777216` = sender CPU1, target CPU0, SGI 0. Flags `0x11` mark the broadcast path and target need bit.

Other CPUs later repeated the same pattern for target CPU0, also sending SGI 0.

2. Did CPU0 receive the IPI?

No receive was recorded. `grep F32I_IPI_RECV` over both run 4 and run 5 returned no events.

The final CPU0 WFI snapshots instead show SGI0 pending in the redistributor while the CPU interface reports no highest-priority pending interrupt:

- Run 4 line 444: `F32I_WFI_HPPIR_PEND` payload `67043329` = `ICC_HPPIR1_EL1 0x03ff` and `GICR_ISPENDR0 low16 0x0001`.
- Run 5 line 431: same payload `67043329`.

Decoded: `ICC_HPPIR1_EL1 == 1023` (spurious/no pending interrupt visible to the CPU interface), while `GICR_ISPENDR0 bit 0 == 1` (SGI0 pending for CPU0).

3. If not received: is PMR too restrictive, is CPU0 already in a higher-priority interrupt, or is SGI routing wrong?

The captured priority state does not support a PMR/RPR blocking explanation:

- Run 4 line 443 and run 5 line 430: `F32I_WFI_PMR_RPR` payload `15728895` = `ICC_PMR_EL1 0xf0`, `ICC_RPR_EL1 0xff`.
- `PMR=0xf0` should allow normal-priority interrupts configured at the default GIC priority. `RPR=0xff` means no active higher-priority interrupt was running.

The suspicious state is SGI visibility/routing/group configuration: SGI0 is pending in CPU0's redistributor, but `HPPIR1` remains 1023 and no IAR-backed SGI receive event occurs.

4. If received but CPU0 did not reschedule: was `need_resched` checked on WFI exit?

The trace does not show receive. It does show that CPU0 kept cycling through the idle-loop WFI instrumentation:

- Run 4 lines 445-450 are a WFI-exit snapshot for iteration 159900; line 451 is the next idle boundary; lines 452-457 are the next WFI-entry snapshot.
- Run 5 lines 439-445 show the same exit-boundary-entry cycle.

Run 5 proves CPU0 can re-enter WFI while the per-CPU need bit is set: line 432 has `F32I_IDLE_BOUNDARY` payload `71657` = CPU0, `need_resched=1`, iteration low bits `6121`. Run 4's final boundary payloads, such as line 451 payload `28829`, decode as `need_resched=0`.

This means there are two related problems to design for:

- The primary observed failure is no SGI receive despite SGI0 pending.
- The idle loop also lacks Linux's explicit "do not enter WFI when `need_resched()` is already true" structure. Run 5 captured that condition.

5. Is CPU0 actually in WFI or a different idle state?

CPU0 is executing the idle-loop WFI path. The WFI point flags alternate between site 1 entry (`flags=0x1`) and site 1 exit (`flags=0x11`) in both runs. `DISR_EL1` and `ISR_EL1` were both zero at the sampled entries/exits:

- Run 4 line 447 and line 460: `F32I_WFI_ISR_DISR ... =0`.
- Run 5 line 441 and line 454: `F32I_WFI_ISR_DISR ... =0`.

So CPU0 is not in a different idle state. It is in the Breenix idle-loop WFI cycle, but interrupts that should drive scheduling are not being acknowledged.

6. Does CPU0's timer show armed/pending state?

Yes for the virtual timer:

- Run 4 line 442 and run 5 line 429: `F32I_WFI_CNT_CTL` payload `262149` = `0x0004_0005`.
- Decoded: `CNTP_CTL_EL0=0x4`, `CNTV_CTL_EL0=0x5`.

`CNTV_CTL_EL0=0x5` means the virtual timer is enabled (`ENABLE=1`), not masked (`IMASK=0`), and its status bit is set (`ISTATUS=1`). The physical timer has status set but is not enabled.

## Diagnosis

The AHCI waitqueue wake path is no longer the break point. The runqueue enqueue and reschedule SGI send happen. The hang point is CPU0 interrupt admission while idle:

- tid 10 is Ready and associated with CPU0 after the IO wake.
- CPU0 remains the idle thread with previous thread 10.
- SGI0 is pending in CPU0's redistributor.
- CPU0's priority mask and running priority do not explain blocking.
- CPU0's CPU interface reports no highest-priority pending interrupt and never records an IAR-backed SGI receive.
- The idle loop can re-enter WFI with `need_resched` already true.

This is a Breenix bug until proven otherwise. The Linux probe validation in `linux-probe-validation.md` shows Linux on the same Parallels ARM64 hypervisor wakes CPU0 from idle reliably.
