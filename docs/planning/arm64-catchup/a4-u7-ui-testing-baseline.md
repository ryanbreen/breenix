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

**FAIL.** One headless HVF boot, Vigil `20261112T193645Z-qemu-9750`, at
check commit `46d291823e610c0b84459a20754e49234684f9f0` above the unchanged
u6 runtime source. Vigil records `aarch64`, `display=false`, `watch=false`
and exit status 1. The launcher records `accel=hvf cpu=host` and one boot
attempt, with four guest CPUs. The pre-boot per-case prediction is effort
ledger line 377.

| Check | Observed result |
|---|---|
| Shared suite | **106 PASS, 2 FAIL, 0 SKIP, 108 total**, elapsed 215009 ms |
| Memory | **PASS**, free 79997 before / 79987 after, 10 retained / allowance 48 |
| Completion | DONE, MEMORY after, MEMORY reclaimed, then READY ui-testing |
| ARM canonical scene | `frames/cross-target` **PASS**, 550 ms |
| WM focus scenario | `proof/wm-focus-scenario` **PASS**, 18813 ms |
| Startup panel | `refused=lease-wait-expired` |
| Final panel | `refused=startup-lease-wait-expired` |
| u4 upload check | **FAIL**, missing/refused/malformed startup panel |
| Host renderer check | **REFUSED**, context 659 first submission lacks creation |

The [ordered capture records](serials/a4-u7-ui-testing.txt) retain 123 lines,
including the 108 case IDs in manifest order, the two failure messages,
retention stamps, panel refusals and completion records. The full serial,
launcher log, writable disk and receiver state remain in the local
`/tmp/a4-u7-arm-ui-testing-20261112/` directory. Vigil retains its own serial,
summary and byte-identical host attestation for this boot.

The focus CASE record at ordered-capture line 116 took **18813 ms** of its
20000 ms deadline: 1187 ms (5.935%) headroom. That already exceeds the
15000 ms maximum required for 25% headroom, before `6ed75a22` changed
snapshot reacquisition. The full retained serial has the same CASE record
at line 4995. These records do not apportion time among checkpoint phases.

No production fix or second boot was made. This failed capture is not ARM
acceptance evidence. No R240 boot, PR or PR merge was performed.

### diagnosis/artifacts-on-mismatch: DHCP-only transport readiness (#2910)

The case failed with `deadline` after 8247 ms. Its 372152-byte bundle has
`result=disk-only:deadline`. The retained transfer stamps are phase 1,
entered 135621 ms, returned 0, deadline 142055 ms; DiagnosticUpload took
6436 ms. The failure is transport, after the contained pixel-mismatch
scenario returned and published its local artifacts.

The producing defect is `libs/libbreenix/src/suite.rs:1503`: evidence
transfer requires a `bootserver` line in `/proc/net/pnp`, waiting up to
200 times 100 ms before opening a socket at line 1520.
`kernel/src/fs/procfs/net.rs:22` emits `bootserver` only for a DHCP lease;
line 35 emits `#MANUAL` for static policy. ARM VirtIO uses static slirp,
not DHCP (`kernel/src/net/mod.rs:1095`). The capture records successful
static network initialization. This configuration cannot satisfy the
sender's lease predicate.

The same sender is called by `upload_diagnostic` at
`libs/libbreenix/src/suite.rs:1468`. Its retention child enters that sender
at `libs/libuitest/src/retention.rs:63`; the shorter case budget expires
at line 93. `userspace/programs/src/uitest_suite/diagnosis.rs:268`
propagates the retention error. The startup panel fails at the same
readiness gate, and the final panel refuses after that startup failure.
The receiver contains route/intent files only, with no upload receipts.
Calling u4's upload checker on the retained capture rejects its startup
panel; case failure remains independently fatal.

Proposed test-first fix: exercise the production transport against a fake
receiver with manual/static configuration. Require successful delivery
and receipts; the old code must fail before socket creation. Make
readiness honor manual configuration while retaining the DHCP acquisition
wait. Keep unconfigured DHCP, malformed/unreadable configuration and
failed-connect/HTTP negatives. Keep the case budget, byte/hash verification
and receipt checks unchanged. Apply the shared repair to panels and bundles.

### proof/wm-stack-scenario: inconsistent observation snapshot (#2911)

The case failed after 2720 ms. It captured window 143 at commit 8,
generation 8; by screen observation the current generation was 9 and
composite 989 named `[(144,5),(143,9)]`. The CASE message truncates after
the projected-entry prefix; no further projected fields are inferred.

The producing race is in
`userspace/programs/src/uitest_suite/proof.rs:720`: each client capture is
stabilized separately and its generation retained. Lines 742-755 then
capture a later screen and compare it with those earlier client samples.
A first client can advance while the other client, tree and screen are
read. There is no restart of the whole observation on that transition.
`proof_wm_checks.rs:7` correctly requires the captured and projected
generations to match; that check must remain strict.

Widget pointer/focus events can legitimately repaint
(`userspace/programs/src/uitest_widget_app.rs:61`, `:100`, `:124`, `:142`).
The capture establishes the 8-to-9 publication race, but does not identify
which event produced generation 9. It does not establish a renderer pixel
defect.

Proposed test-first fix: force a new front publication after the first
client capture and before the screen read in a deterministic production
checkpoint fixture. The current acquisition must fail; repaired causal
settling/capture coordination must acquire a consistent client/screen set
within the existing deadline. Re-capture a superseded snapshot. Keep a
permanently stale composite negative, exact front/tree/screen comparisons
and stacking expectations. Do not accept generation 9 against captured
generation 8, add sleeps or relax the predicate.

### Host renderer attestation: silent event eviction (#2912)

The launcher's unchanged checker refused
`first-submit-3d of context 659 has no context-create for that context before it`.
The attestation has exactly 128 records. Its first record is that submission,
and no creation for context 659 remains. Its strings name ANGLE/Metal;
the attestation still fails, so hardware rendering is not accepted.

The producing defect is in the pinned external QEMU fork at
`160f45890824de1270a2f1e84b9d02a12b2219d2`,
`hw/display/virtio-gpu-virgl.c:586` and `:648-657`: the producer caps its
queue at 128 events, removes one oldest event at a time, and rewrites the
file from that tail. This can discard creation ancestry and earlier evidence.
`scripts/check-host-renderer.py:65` correctly refuses the unmatched event.
The fix belongs in the external fork, with no GPL source added to Breenix.

Proposed test-first fix: drive the production attestation writer through
more than 128 events and judge the output with the unchanged checker. The
current writer must fail. Preserve the complete boot's evidence using
bounded memory and explicit refusal on storage overflow, rather than
silently evicting evidence. Keep missing-creation and early-software-renderer
negatives even after many later hardware events. Do not weaken the checker.

### Capture identity and branch freshness

| Original local artifact | SHA-256 |
|---|---|
| serial.txt | `40a9cb22758222d3a43ea94252e8174c7493b370b9c4766d33655e43fcabe9ba` |
| host-renderer.json | `4e037cd7e3d8fe63e81a52c33a4771095bddb01fb453ed1c05c46f0790bd0b9f` |
| launcher.log | `fd077e82f8febe1fe401df9de96d14cb37d51f5749887db6f7cde84e9a485358` |

After the failed baseline, the repository freshness instruction required
merging current origin/main before push. Merge `c2781bb4` brings in
`5ee353ad` without conflicts. The report's source citations and build/boot
identity above refer to the captured `46d29182` tree. The merged runtime
was not rebuilt or booted in this measurement-only unit, and this report
makes no final-head acceptance claim.

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
