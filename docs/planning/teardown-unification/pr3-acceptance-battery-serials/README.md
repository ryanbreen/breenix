# T3-G PR3 (PR-A) acceptance-battery serials — every non-GREEN boot, preserved

These are the raw serial logs of every non-GREEN boot in the PR-A acceptance battery run on
`fix/producer-corruption-family` @ `14272b9a` (330 QEMU boots: full-test, prod-profile,
service-sequence 25/profile, clean 100/profile, starved 50/profile, strict 3x20). Round-1 kept them
only in a temp directory; campaign law is that a preserved serial is what turns a flake into a filed
bug, so they live here.

Every boot below came from `docker/qemu/run-aarch64-service-sequence-gate.sh` with a kernel built
from `14272b9a` for the soft-float target `aarch64-breenix-kernel.json` with `--features boot_tests`,
booted as

```
qemu-system-aarch64 -M virt,gic-version=3 -cpu <profile> -m 512 -smp 4 \
  -kernel <kernel> -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 -drive if=none,id=ext2,format=raw,file=<ext2 image copy> \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 -serial file:<serial>
```

with `<profile>` = `max` or `cortex-a72`. The battery totals were 196/200 GREEN on the clean legs,
100/100 GREEN starved, 50/50 GREEN service-sequence, 60/60 strict.

| file | run | face | attribution |
|---|---|---|---|
| `clean-cortex100-boot41-635-far-eq-elr.txt` | clean, `cortex-a72`, boot 41/100 | `[INSTRUCTION_ABORT] FAR=ELR=0xffff000041139200 ESR=0x8600000e IFSC=0xe from_el0=0`, `[FATAL_REGS] cpu=2 ... x30=0xffff000041139200`, `resume_pc_refusals=0` | **#635**, reopened. This is the production face of the family on this branch, and it did **not** traverse a resume-PC slot: the address is `.bss` (`DISPATCH_TRACE+0x40`), outside `[__kernel_text_start, __kernel_text_end)`, so every unified consumer would have refused it and emitted `[RESUME_PC_REFUSED:...]` — none did, and the postmortem census on this boot reports `kstack=0` and `other=0` for every source on every CPU. `x30` holds the faulting address: a live `ret`/`blr` through a corrupted `x30`, not an exception return. |
| `clean-cortex100-boot87-613-disagreeing-records.txt` | clean, `cortex-a72`, boot 87/100 | two disagreeing instruction-abort records for one fault: `far/elr/esr = 0xffff000041139200 0x0 0x8600000e` and `0xffff000041139200 0xffff000040509540 0x8600000e`; register file made of AArch64 instruction words (`x24=0xd538d088b40002a8` = `mrs x8, tpidr_el1`, `x25=0xf9400108b40002e8` = `ldr x8,[x8]`) | pre-adjudicated **#613** (disagreeing abort records) by field signature; the callee-saved-registers-restored-from-kernel-text shape is corroborating evidence on **#635**. |
| `clean-cortex100-boot97-far0x2-null-data-abort.txt` | clean, `cortex-a72`, boot 97/100 | `[DATA_ABORT] FAR=0x2 ELR=0xffff0000404e80a0 ESR=0x96000005 DFSC=0x5 TTBR0=0x10000440dd000 from_el0=0`, `[FATAL_REGS] cpu=0 spsr=0x600000c5 sp=0xffff0000431ffd90` with `x9=0x2 x16=0x2` | **#641** — a new field signature, matched by nothing filed (not #633 `PC_ALIGN`, not #635 `FAR==ELR`, not #637 EL0-at-kernel-addr, not #576/#626 zero-PC, not #612's `FAR=0x292` region, not #639/#640). Filed rather than folded into any tolerated bucket. |
| `clean-max100-boot26-555-softirq.txt` | clean, `max`, boot 26/100 | `[TEST:interrupts:softirq_aarch64:FAIL:raise_softirq did not set pending bit on ARM64]` | pre-adjudicated **#555** softirq bucket, at its documented ~1% ceiling. |

The `[RESUME_PC_REFUSED:` count was **0** on every one of the 330 boots, on every profile, including
these four: nothing this PR added fired in production, and nothing this PR added contained the
faults above.

Leg-by-leg self-test serials for the same branch (red-before / green-after / disarmed / mutated) are
in the sibling directory `../633-635-637-serials/`.
