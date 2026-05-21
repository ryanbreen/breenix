# Turn 6 Baseline DATA_ABORT Pre-Existence Test

Status: COMPLETE

Turn 6 ran 5 consecutive 120s Parallels aarch64 boots on the unmodified Turn 3
baseline at commit `18c88a01`. The Turn 5 xHCI completion conversion was not
applied.

## Baseline Confirmation

```text
18c88a01 kernel/userspace: CPU0 liveness fix + /proc/xhci/counters
361c5a6c docs: comprehensive polling inventory + Linux comparison for elimination Ralph
ac93a078 Merge pull request #348 from ryanbreen/investigation/driver-comment-honesty-pass
```

No source-file modifications were present before the Turn 6 build or boot loop.
Uncommitted Turn 5 review artifacts were present, but no source files were dirty.

## Build Evidence

- `turn6-artifacts/build-aarch64.log`: clean
- `turn6-artifacts/build-userspace.log`: clean
- `turn6-artifacts/build-ext2.log`: clean
- `turn6-artifacts/build-efi.log`: clean
- Warning/error grep across build logs and per-boot `parallels-run.out` files:
  empty

## Five-Boot Aggregate

| Boot | CPU0 final | MSI events | IRQ entries | Lock contended | Failure signatures | DATA_ABORT lines | PID1 reached |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 91868 | 1 | 1 | 0 | 0 | 0 | yes |
| 2 | 84859 | 0 | 1 | 1 | 0 | 0 | yes |
| 3 | 84385 | 1 | 1 | 0 | 0 | 0 | yes |
| 4 | 88436 | 1 | 1 | 0 | 0 | 0 | yes |
| 5 | 78378 | 1 | 1 | 0 | 0 | 0 | yes |

Raw aggregate:

```text
1 cpu=91868 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
2 cpu=84859 msi=0 irq=1 lock=1 failures=0 data_abort=0 pid1=yes
3 cpu=84385 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
4 cpu=88436 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
5 cpu=78378 msi=1 irq=1 lock=0 failures=0 data_abort=0 pid1=yes
```

## DATA_ABORT Count

Baseline DATA_ABORT count: 0 of 5 boots.

The Turn 5 boot-4 `DATA_ABORT` signature was not reproduced in this Turn 3
baseline sample.

## Artifact Index

- `turn6-artifacts/aggregate.tsv`
- `turn6-artifacts/build-aarch64.log`
- `turn6-artifacts/build-userspace.log`
- `turn6-artifacts/build-ext2.log`
- `turn6-artifacts/build-efi.log`
- `turn6-artifacts/build-warning-grep.log`
- `turn6-artifacts/boot-*/parallels-run.out`
- `turn6-artifacts/boot-*/parallels-boot.log`
- `turn6-artifacts/boot-*/failures.txt`
- `turn6-artifacts/boot-*/data-abort.txt`
- `turn6-artifacts/boot-*/cpu0-final.txt`
- `turn6-artifacts/boot-*/xhci-counters.txt`
- `turn6-artifacts/boot-*/pid1.txt`
- `turn6-artifacts/boot-*/parallels-screenshot.png`
