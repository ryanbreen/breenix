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

Pending the single baseline boot. Record failing assertion text and suspected
producer locations, grouped as input, pointer, performance (with values),
lifecycle/clipboard/recovery, and other. Suspected causes are hypotheses,
requiring later deterministic reproduction.
