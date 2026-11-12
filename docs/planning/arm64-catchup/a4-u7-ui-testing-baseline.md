# A.4 unit 7: ARM64 UI-testing baseline

## Check registered before the boot

Start from u6, `1da26f19`. This unit changes no runtime behavior. Its check
is the existing `docs/suites/ui-testing.json`, with its 108 cases and
unchanged assertions. Record a per-case PASS prediction in the effort
ledger before the single boot. The report will be completed from that
boot's capture; no fix or second boot belongs to this unit.

Build the production ARM64 kernel, userspace and disk at the chain head:

```sh
export CARGO_BUILD_JOBS=4
scripts/boot-interactive.sh --mode suite --suite ui-testing --gpu-gl --no-display --build-only
```

The launcher applies background policy to build subprocesses. Then run
once, outside background build policy, through the mac-boot slot:

```sh
scripts/boot-interactive.sh --mode suite --suite ui-testing --gpu-gl --no-display --no-build
```

`--no-build` reuses the completed build. Require HVF, four guest CPUs,
headless SPICE GL and the ordinary launch priority policy. No debugger.

## Code-derived question and prediction

Does the assembled ARM launcher deliver the existing shared UI suite's
results and evidence over slirp, using the ARM canonical pin? The host
checks reported by u1-u6 cover the component mechanisms; this boot checks
their guest integration. Existing serial diagnostics, receiver receipts
and host renderer attestation provide the observations.

- Predict 108 PASS, no FAIL or SKIP, in manifest order.
- `scripts/boot-interactive.sh:380` starts the standalone receiver and
  stages its private route on the writable disk. `scripts/qemu-evidence.py:88`
  addresses the host through slirp. Predict received evidence rather than
  a missing/refused route or disk-only retention.
- `userspace/programs/src/uitest_suite/canonical_cases.rs:30` selects the
  provenance row by architecture. Predict `frames/cross-target` PASS using
  the `aarch64` / `arm64-qemu` row and its committed pin.
- `libs/libbreenix/src/suite.rs:1262` captures the startup panel;
  `libs/libbreenix/src/suite.rs:1412` captures the final panel after memory
  reclamation. Require u4's `scripts/suite-verdict.py:95` upload checks:
  panel anchors, receiver receipts, reassembled bytes, sizes and SHA-256,
  plus successful bundle receipts. Both panels must be verified.
- Require MEMORY reclaimed=PASS and, under U19, a complete
  `SUITE_SEQUENCE READY ui-testing` after DONE and MEMORY. The staged
  launch is standalone, so a multi-suite DONE is not its completion marker.
- Require `scripts/check-host-renderer.py` to accept the boot's actual
  ANGLE/Metal renderer. Judge the result from the Vigil record, the u4
  verdict and this renderer check.

## Result

Pending the single baseline boot.

## Build record

The ARM production build completed successfully at the u6 source head.
The FP/SIMD guard passed. Its only warning was the pinned Rust-core
future-incompatibility advisory covered by G35(2); no new warning appeared.
The build log is retained locally as `/tmp/a4-u7-build.log`.

| Artifact | SHA-256 |
|---|---|
| ARM production kernel | `0dec4ed125bb9e373ea59125451aca37ac98ce0ec1186987001d1482b5ba9df2` |
| suite-ui-testing release and installed ELF | `305e700ff436db014210df021c3fe329b4788d12c90edfa2191154161a12d188` |

u6's scoped x86 release build is retained as reported in its ledger result;
this unit adds documentation only and does not repeat that build.
