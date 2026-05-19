# Turn 6 Lock-Hold Attribution

Status: COMPLETE.

## A. Counter And Emitter Diff

The GPU PCI lock wrapper now records memory-only contiguous hold duration on every successful acquire and every guard drop:

```diff
+pub static GPU_PCI_LOCK_HOLDER_TID: AtomicI64 = AtomicI64::new(-1);
+pub static GPU_PCI_LOCK_ACQUIRED_AT_NS: AtomicU64 = AtomicU64::new(0);
+pub static GPU_PCI_LOCK_MAX_HOLD_NS: AtomicU64 = AtomicU64::new(0);
+pub static GPU_PCI_LOCK_MAX_HOLD_HOLDER_TID: AtomicI64 = AtomicI64::new(-1);
+
+gpu_pci_lock_record_acquire();
+gpu_pci_lock_record_release();
```

The freeze-watch kthread emits the attribution snapshot every 30 seconds:

```text
[gpu-pci-lock-attrib] max_hold_ms=<N> max_hold_holder_tid=<T> rescues=<R>
```

`rescues` is backed by a new scheduler counter that increments for queue-empty and timer rescue paths, instead of relying on rate-limited serial rescue markers.

## B. Serial Excerpt

```text
[gpu-pci-lock-attrib] max_hold_ms=22 max_hold_holder_tid=-1 rescues=0
[gpu-pci-lock-attrib] max_hold_ms=82 max_hold_holder_tid=13 rescues=7
[gpu-pci-lock-attrib] max_hold_ms=82 max_hold_holder_tid=13 rescues=12
[gpu-pci-lock-attrib] max_hold_ms=82 max_hold_holder_tid=13 rescues=13
[gpu-pci-lock-attrib] max_hold_ms=82 max_hold_holder_tid=13 rescues=13
[gpu-pci-lock-attrib] max_hold_ms=83 max_hold_holder_tid=13 rescues=15
[gpu-pci-lock-attrib] max_hold_ms=83 max_hold_holder_tid=13 rescues=21
[gpu-pci-lock-attrib] max_hold_ms=83 max_hold_holder_tid=13 rescues=22
```

Final freeze-watch sample:

```text
[freeze-watch] uptime_ms=220465 submits=131321 completes=131324 fails=0 last_completion_ms=220462 fps_last_5s=179 ... gpu_pci_lock=ok
```

Fatal markers were clear in the single boot:

```text
stuck_tid13=0
softlock=0
cpu0=0
far=0
panic=0
```

## C. max_hold_ms Evolution

| uptime_ms | max_hold_ms | holder_tid | rescues | fps | completes | freeze_gpu_lock |
| ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1804 | 22 | -1 | 0 | 56 | 149 | ok |
| 35361 | 82 | 13 | 7 | 202 | 19674 | busy |
| 65374 | 82 | 13 | 12 | 187 | 37683 | busy |
| 95396 | 82 | 13 | 13 | 202 | 54311 | ok |
| 125410 | 82 | 13 | 13 | 209 | 73103 | ok |
| 155428 | 83 | 13 | 15 | 210 | 92078 | ok |
| 185446 | 83 | 13 | 21 | 201 | 110573 | ok |
| 215461 | 83 | 13 | 22 | 193 | 128627 | ok |

The sample-based freeze-watch busy aggregation still reported:

```text
sample_busy_max_ms=5003
gpu_pci_lock=busy samples: 17
gpu_pci_lock=ok samples: 43
```

## D. Holder

The maximum observed contiguous hold was 83 ms, held by tid 13. This is the BWM render thread, but it is not a 5 second lock hold.

## E. Decision

`max_hold_ms` stayed below 100 ms for the full 220 second active window. Per the Turn 6 decision tree, the Turn 5 `gpu_pci_lock_busy_gt_5s` failure is a harness/metric aggregation bug: freeze-watch samples can observe the lock as busy across adjacent samples while the lock is actually being acquired and released repeatedly by separate short holds.

The strict 5 second criterion should use the kernel-reported contiguous hold metric, not consecutive sampled `gpu_pci_lock=busy` strings.

Secondary finding: the rescue counter reached 22 while only 5 `queue_empty rescue_tid=13` serial lines appeared, because those serial lines are rate-limited. Turn 7 should avoid using serial rescue marker count as the source of truth if rescue count remains part of the gate.

## F. Turn 7 Scope

Fix the stress harness metric:

- Parse `[gpu-pci-lock-attrib] max_hold_ms=<N>` and use it for the 5 second lock-hold gate.
- Keep sampled `gpu_pci_lock=busy` as a liveness clue, not as contiguous hold evidence.
- Parse the scheduler rescue counter from attribution lines if the gate wants true rescue count; otherwise explicitly keep the old rate-limited serial-marker metric and label it as such.

No virtio-gpu polling fallback was added, and the interrupt-driven completion path remains load-bearing.

## Validation

```text
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
Finished release profile with no warnings in compiler output.
```

Format note: whole-repo `cargo fmt --check` still fails on pre-existing unrelated formatting/trailing-whitespace drift, including files outside this turn and gold-master/prohibited areas. `git diff --check` on the Turn 6 diff is clean.
