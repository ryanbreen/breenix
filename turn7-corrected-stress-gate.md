# Turn 7 Corrected Stress Gate

## A. Harness Diff

Turn 7 copied the Turn 4 stress harness to `turn7-artifacts/run_5boot_corrected.sh` and changed the pass/fail metrics to use kernel counters emitted by `[gpu-pci-lock-attrib]`.

Key changes:

- Parse the latest `[gpu-pci-lock-attrib]` line after the active rendering window.
- Gate on `max_hold_ms <= 200` instead of sample-based `max_busy_ms`.
- Gate on `rescues <= 30` instead of rate-limited `rescue_tid13` marker counts.
- Keep `sample_busy_max_ms` in `metrics.tsv` only as a liveness/debug clue.
- Include `max_hold_ms`, `max_hold_holder_tid`, `rescues`, and `sample_busy_max_ms` in per-boot result files and aggregate output.

The harness-only commit is:

```text
ef77cbb5 chore(harness): switch 5-boot gate to kernel-counter metrics
```

## B. Five-Boot Table

| Boot | Status | max_uptime_ms | final_fps | final_completes | max_hold_ms | rescues | sample_busy_max_ms | Fatal markers |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| boot-1 | pass | 308515 | 103 | 110792 | 46 | 5 | 25021 | 0 |
| boot-2 | pass | 221457 | 169 | 114446 | 45 | 28 | 10005 | 0 |
| boot-3 | pass | 221453 | 166 | 115814 | 44 | 28 | 15008 | 0 |
| boot-4 | pass | 292463 | 160 | 139793 | 89 | 26 | 15010 | 0 |
| boot-5 | pass | 221533 | 151 | 105947 | 89 | 21 | 20021 | 0 |

Fatal markers are the combined gate signals: `stuck_tid13`, softlock banners, CPU0 timer regression, FAR=0xccd, panic/exception, and virtio-gpu polling markers. All were zero in all five boots.

## C. Aggregate Metrics

```text
boot-1: pass + max_hold_ms=46 rescues=5 fps=103 + reason=pass
boot-2: pass + max_hold_ms=45 rescues=28 fps=169 + reason=pass
boot-3: pass + max_hold_ms=44 rescues=28 fps=166 + reason=pass
boot-4: pass + max_hold_ms=89 rescues=26 fps=160 + reason=pass
boot-5: pass + max_hold_ms=89 rescues=21 fps=151 + reason=pass
overall: pass
max_hold_ms: distribution = 46,45,44,89,89, max across all = 89
rescues: distribution = 5,28,28,26,21, max across all = 28
fps_at_end: min=103, max=169, mean=149.8
completes_at_end: min=105947, max=139793, mean=117358.4
```

The old sample-based busy metric was still high in several boots, topping out at 25021 ms, but Turn 6 proved that value is accumulated sampling, not contiguous `gpu_pci_lock` hold time. The corrected contiguous hold gate maxed at 89 ms.

The rescue counter remains nonzero: 5, 28, 28, 26, and 21 events per boot. This is bounded under the Turn 7 gate, but it is a real follow-up signal and is called out in the PR body.

## D. PR URL

https://github.com/ryanbreen/breenix/pull/343

## E. Status

COMPLETE. The corrected 5-boot Parallels gate passed 5/5, the branch was pushed, and PR #343 is open.
