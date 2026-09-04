# #789 — the mutation probe and its matched baseline

Twenty production-profile boots on this Mac, two QEMUs at a time, same runner
(`../tools/run_n.sh`), same ext2 fixture, same 80 s/130 s windows: ten of the
unmodified branch kernel and ten of a kernel carrying one scratch edit.

## The scratch edit

Applied to `kernel/src/task/scheduler.rs::is_current_idle_thread`, built, run,
then **reverted**. It is not in any commit; this file is the record of what was
run.

```rust
pub fn is_current_idle_thread() -> Option<bool> {
    // SCRATCH PROBE (#789, never committed): mask interrupts across the
    // acquisition so an IRQ cannot land inside this CPU's own critical section.
    without_interrupts(|| {
        if let Some(scheduler_lock) = try_lock_scheduler() {
            if let Some(scheduler) = scheduler_lock.as_ref() {
                return scheduler
                    .current_thread_id_inner()
                    .map(|id| id == scheduler.idle_thread_id());
            }
        }
        None
    })
}
```

That hunk was the whole edit. The probe is a causality check on the mechanism
in `../../789-RCA-2026-09-04.md`, not a proposed patch.
claim-lint:ok: 1 of 1 changed function, quoted verbatim above.

## Results

| lane | kernel | verdicts |
|---|---|---|
| `BL1` | unmodified | wedge, wedge, pass, pass, wedge |
| `BL2` | unmodified | wedge, pass, wedge, wedge, wedge |
| `P1` | + masking probe | pass, pass, pass, pass, pass |
| `P2` | + masking probe | pass, pass, pass, pass, pass |

**Unmodified 3/10 reached `bsshd: listening`. With the probe, 10/10.** The
baseline rate matches #789's own measurement of 3/8 on this head.

Fisher's exact test on 3/10 versus 10/10: one-sided p = 0.001548, computed by

```
python3 -c "from math import comb; print(comb(13,3)*comb(7,7)/comb(20,10))"
```

## The CPU signature

`run_n.sh` samples the QEMU process's `%cpu` at the moment it scores each boot.
7 of the 7 wedged boots read 393.8–398.6; 13 of the 13 passing boots read
163.4–193.1. The four vCPUs are spinning, not halted in WFI — which is the
discriminating experiment #789 named.
claim-lint:ok: 20 of 20 boots, the four run logs in this directory.

## Files

* `probe-P1-log.txt`, `probe-P2-log.txt` — per-boot verdict + QEMU `%cpu`, probe arm
* `baseline-BL1-log.txt`, `baseline-BL2-log.txt` — same, unmodified arm
* `serials/<lane>-boot<n>.txt` — the 20 guest serials, one per boot
