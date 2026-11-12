# A.3 unit 1: ARM64 window-manager baseline

## Check registered before the boot

Baseline source: `9afc665ce223366d53bc7c92707e0503171c8f20`.
This unit changes no runtime behavior. Its check is the existing
`docs/suites/window-manager.json` suite, implemented by
`userspace/programs/src/suite_window_manager.rs` and `wm_suite/`.
Keep its assertions and limits unchanged; no additional regression test is
needed for a measurement-only unit.

Run once, after recording the prediction in the effort ledger:

```sh
scripts/boot-interactive.sh --mode suite --suite window-manager --gpu-gl --no-display --no-build
```

`--no-build` reuses the completed release build of the unchanged baseline:
production ARM64 kernel, bwm, suite-window-manager, wm-client_test and disk.
The build's kernel FP/SIMD guard passed. The pinned Rust core future
incompatibility notice is the existing notice identified by G35(2).

Judge the boot from its Vigil record, the launcher's
`scripts/check-host-renderer.py` result and the standalone completion shape:
`SUITE window-manager DONE`, followed by the after/reclaimed MEMORY records,
then `SUITE_SEQUENCE READY window-manager` (U19). A failing baseline remains
failing. Do not boot a second time or repair source in this unit.

## Code-derived prediction

The question is which existing ARM64 gaps actually fail the shared suite,
and which measured values distinguish input routing from presentation cost.
Existing case diagnostics, input receipts and failure-only pipeline exports
provide the observations; no diagnostic code is added.

- Input: the suite launcher declares `qmp` at
  `scripts/boot-interactive.sh:254-256`, arming the PL011 host channel at
  `kernel/src/main_aarch64.rs:333-343`. Predict host-key delivery when that
  channel works. Driver replay cases may fail ENODEV at
  `kernel/src/input/control.rs:63` if no driver registered; the ARM replay
  also refuses an uninitialized instance at
  `kernel/src/drivers/virtio/input_mmio.rs:856-863`.
- Pointer: `userspace/programs/src/bwm/input.rs:300-330` normalizes and groups
  contiguous axes from one read. If the axes arrive in separate reads,
  exact-coordinate, capture, ordering and drag checks may fail. The boot
  distinguishes that hypothesis from functioning batching.
- Performance: `userspace/programs/src/bwm/pace.rs:9` composes immediately
  for period 0. Completion-driven presentation may therefore miss the
  existing input/present latency or frame-floor limits at
  `userspace/programs/src/wm_suite/cases.rs:1310-1507`. Report measurements,
  without guessing their numeric values from the code.
- Lifecycle, clipboard and recovery: no specific ARM-only defect is
  established by this reading. Predict pass where setup/input succeeds;
  inspect restart adoption and clipboard peer/serial checks at
  `userspace/programs/src/wm_suite/cases.rs:1070-1088,1161-1175` if they fail.
- Other: predict startup, composition, stacking and liveness pass where
  setup succeeds. Distinguish dependent setup errors from independent
  assertions. RSS growth remains limited to 64 KiB at
  `userspace/programs/src/wm_suite/cases.rs:1290-1298`.

## Result

One headless ARM64 QEMU HVF boot, Vigil
`20261111T054423Z-qemu-34249`, recorded at `46c2abd3` (the check-only commit
above the unchanged baseline source): **77 passed, 4 failed, 0 skipped, 81
total**, elapsed 440026 ms. The launcher and Vigil both record failure.
The launcher's host-renderer check accepted ANGLE/Metal (1025 records).
No second boot, source fix, x86 boot or PR was performed.

The ordered [suite records](serials/a3-u1.txt) retain the case messages and
measurements. Their 81 case IDs match the manifest in order, and their four
failing IDs match Vigil's summary. The complete private capture, Vigil
metadata/summary, host-renderer attestation, build log and artifact hashes
are retained in the local acceptance directory for `arm-a3-u1-20261111`.

Memory reclamation passed: free pages 84640 before, 84630 after, 10 retained
against allowance 48. `SUITE_SEQUENCE READY window-manager` follows DONE,
MEMORY after and MEMORY reclaimed, satisfying U19's standalone shape.
The capture's evidence startup/final-panel records say `skipped=no-route`;
this baseline does not add the ARM evidence route planned for A.4.

### Input

No failing cases: input-core 9/9, input-devices 2/2 and focus 9/9 passed.
Both production driver replay paths succeeded, so the predicted ENODEV gap
did not appear. Host-key delivery was exercised successfully.

### Pointer

No failing cases: pointer 6/6 and geometry 6/6 passed, as did
input-core/virtual-pointer. The split-axis concern did not manifest in this
boot. This result does not establish that reads can never split a report.

### Performance

| Case | Measured result | Existing limit | Verdict |
|---|---|---|---|
| typing-latency | median 10.785 ms, max 16.050 ms | median 20 ms, max 50 ms | PASS |
| pointer-latency | median 6.958 ms, max 11.695 ms | median 20 ms, max 50 ms | PASS |
| present-latency | median 2.118 ms, max 3.630 ms | median 16 ms, max 33 ms | PASS |
| frame-floor | 376 Hz; 1881 intervals / 4996026333 ns | at least 60 Hz | PASS |
| idle-cpu | 0 percent (reported integer) | at most 2 percent | PASS |
| latency-under-animation | median 12.873 ms, max **281.547 ms** | median 33 ms, max 100 ms | **FAIL** |
| clipboard-latency | max 6.606 ms | max 50 ms | PASS |

`performance/latency-under-animation`, case duration 13964 ms, fails with
`input-to-composited-ack latency exceeds the GUI plan bound`
(`userspace/programs/src/wm_suite/cases.rs:1501-1504`). Suspect unbounded
competition between synchronous composition/validation and new input:
VirGL reports period 0 at `kernel/src/gpu/dev.rs:42-47`, which bypasses the
response hold at `userspace/programs/src/bwm/pace.rs:9`. Deferred validation
also performs synchronous frame readback in
`userspace/programs/src/bwm/chrome.rs:45-68` and
`bwm/compositor.rs:481-547`. These are candidate paths, not an attribution
of the maximum sample. The failure export retains 256 pipeline rows and
the boot-lifetime fine-wake maximum of 493598291 ns; that maximum is not
correlated to this input sample. Pacing follow-ups remain #2761 and #2763.

### Lifecycle, clipboard and recovery

Lifecycle 9/9 and clipboard 6/6 passed. Recovery passed 3/5, including
clients-notified, interaction-after-restart and stack-restored.

| Failing case | Assertion text | Case duration |
|---|---|---|
| recovery/restart-adopts | `bwm first completed desktop: no acknowledgment within 4000 ms` | 6388 ms |
| recovery/supervisor-restarts | `bwm composite of client 83 commit 4: no acknowledgment within 2000 ms` | 9209 ms |

For restart-adopts, `userspace/programs/src/wm_suite/cases.rs:1070-1076`
freezes the surviving client, kills bwm, then starts its replacement. The
failure is the replacement desktop observation at
`wm_suite/desktop.rs:448-460`, before the retained client's pixel assertion.
Suspect restart presentation cost or fresh-context visibility:
`bwm.rs:256-341` performs fill/title/full-frame checks before composing
initial clients at `bwm.rs:375-377`. The failure window records
`bwm-check-us` 1300246 and 785268. Those durations do not distinguish slow
startup from an observation/context problem.

For supervisor-restarts, `wm_suite/cases.rs:1127-1131` observes a replacement
bwm child before checking survivor content. The failed wait is
`wm_suite/desktop.rs:496-517`. Suspect that child existence precedes
completion of the same startup/adoption path; the replacement's recorded
checks took 10727 and 1079134 us. Retained-front publication at initial
composition also needs inspection. The existing-windows and other restart
passes show that adoption works in those scenarios.

The suite's CASE messages append truncated process output after these
assertions; the exact emitted records remain in `serials/a3-u1.txt`.

### Other

`liveness/memory-bounded`, case duration 32857 ms, fails with
`bwm resident memory grew more than 64 KiB after warmup` at
`userspace/programs/src/wm_suite/cases.rs:1296-1298`. Measured RSS growth is
**9248 KiB**, limit 64 KiB, after 60 warmup epochs and 600 measured epochs
with eight clients (`cases.rs:1273-1288`). Suspect retained resident
mappings from per-frame allocations in `bwm/compositor.rs:289-323` or
readback buffers at `:498-536`, or ARM mapping/RSS accounting at
`kernel/src/fs/procfs/mod.rs:1808-1819` and
`kernel/src/memory/process_memory.rs:1710`. The intended unmap path is
`libs/libbreenix-libc/src/stdlib/malloc.rs:69-85`; its bounded small-block
cache (`alloc_cache.rs:6-7`) alone cannot explain 9248 KiB. Attribution
requires a deterministic allocation/page-lifetime check in a later unit.
The end-of-suite memory reclamation pass does not erase this live-process
RSS failure.

Startup 6/6, composition 6/6, stacking 5/5 and the other liveness 4/4 cases
passed. Recovery/RSS follow-up is [#2872](https://github.com/ryanbreen/breenix/issues/2872),
separate from the existing pacing work. No failing assertion, threshold or wait was changed.
