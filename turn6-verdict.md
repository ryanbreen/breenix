# Turn 6 Verdict

Status: COMPLETE

Result: 0 of 5 Turn 3 baseline boots hit the `DATA_ABORT` signature.

Verdict: the Turn 5 xHCI IRQ-driven completion conversion is not cleared as
innocent by baseline evidence. The Turn 5 timing is exposing a race that did not
appear in this 5-run baseline sample.

## Basis

Turn 5 had 1 failure in 5 boots:

```text
[DATA_ABORT] FAR=0x2a0 ELR=0xffff0000401bfe50 ESR=0x96000005 DFSC=0x5 TTBR0=0x1000044094000 from_el0=0 cpu=2
  x19=0x60 x20=0xffff0000401e7268 x8=0x2110000 x9=0x2110000 x29=0x3a x30=0xffff0000401bfe50 sp=0xffff000054265ab0 tid=14 name=bwm
[DATA_ABORT] kernel-mode fault, deferring process cleanup
```

Turn 6 ran the unmodified Turn 3 baseline for the same 5-boot sample size:

```text
1 cpu=91868 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
2 cpu=84859 msi=0 irq=1 lock=1 failures=0 data_abort=0 pid1=yes
3 cpu=84385 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
4 cpu=88436 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
5 cpu=78378 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
```

Since the baseline reproduced 0 DATA_ABORT signatures, the empirical result
matches the Turn 6 `0/5` branch: Turn 5's IRQ timing is exposing a new race or a
rare race that did not reproduce on the baseline sample.

## Proposed Turn 7

Restore the Turn 5 conversion patch and debug the AHCI completion / BWM
`DATA_ABORT` path under that timing:

- Reapply `turn5-artifacts/turn5-attempted.diff`.
- Prefer GDB or nonintrusive trace analysis, not logging in hot interrupt paths.
- Break or trace near the kernel-mode abort address
  `ELR=0xffff0000401bfe50`.
- Correlate the abort with AHCI state from the Turn 5 failure:
  `AHCI arm port=1 cmd=1185 isr port=1 cmd=1185 waiter_tid=11`.
- Determine whether the xHCI conversion changes scheduling enough to expose an
  existing AHCI/process cleanup race, or whether the conversion itself violates a
  cross-subsystem assumption.

No source changes were made in Turn 6.
