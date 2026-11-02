# Screen proof contract <!-- claim-lint:ok: Normative contract specified in docs/gui/screen-proof.md, not a measured result. -->

<!-- claim-lint:ok: Normative contract specified in docs/gui/screen-proof.md, not a measured result. -->
Screen proof adds checkpoints and a separate verdict alongside the GPU and BreenGL suite verdicts. Existing cases, expected values, thresholds and reviewed references remain unchanged. This is the `screens` milestone of E22 on both architecture tabs and a prerequisite for E22 `frames`. G29 adopts the shared picture table and the derived native display-output evidence described below. This document specifies the intended contract; implementation and acceptance belong to subsequent units.

## Observation and records

<!-- claim-lint:ok: Observation contract here; scan origins from kernel/src/gpu/virgl/mod.rs set_scanout, kernel/src/gpu/svga/mod.rs set_scanout and kernel/src/gpu/gk208/modeset.rs Viewport::methods. -->
A checkpoint observes exactly the mode-sized rectangle scanned out from the bound resource. A larger binding is valid; a smaller binding or unknown scan origin is an error. All current backends program scan origin (0,0): VirGL SET_SCANOUT, SVGA screen binding and GK208 viewport-in. The record's rectangle must match that origin and `CURRENT_MODE`; target dimensions describe the binding and must contain the rectangle. The SCREEN line records both mode and binding dimensions.

<!-- claim-lint:ok: Endpoint semantics specified here and implemented by kernel/src/gpu/mod.rs observe_screen and read_back. -->
Checkpoint readback uses the root-only, suite-only `/dev/screen-evidence` binary request: exactly 32 bytes, `SCREENRB` followed by little-endian u64 pixel pointer, buffer length and record pointer. Screen-only waits for restoration, pending presentation and outstanding target writes share a five-second preparation deadline and return a SCREEN readback error on timeout. Transfers retain the ordinary device watchdog and resource holds. Ordinary observation waits keep their existing behavior. The GPU core pins the binding, observes the visible mode rectangle with its ordinary poison/transfer checks, and writes canonical pixels plus record v2. This pixel-only endpoint carries no composition provenance (flag bit 1, composite serial and window entries stay zero), so the number of windows cannot truncate its record. Ordinary READBACK and SCANOUT_READBACK retain their complete-provenance requirement and reason-11 refusal above 16 windows, with untouched outputs. No syscall operation or reserved record field changes.

<!-- claim-lint:ok: Normative checkpoint eligibility here; kernel/src/gpu/display.rs Transition::observe serializes pending presentation. -->
A transiently unknown `MODES` binding proceeds to the pinned observer, which waits for outstanding presentation before deciding whether the record belongs to the case. A settled unknown binding is an observation error, never silent absence. A known empty binding has no scanout to observe: `display_reset` clears the binding and marks it known after confirmed device reset (`kernel/src/gpu/display.rs`). The screen endpoint returns errno 61 for that state, checked under the pinned transition, and writes a `SCREENER` status into the first 32 bytes of the record buffer without writing pixels. This does not alter ordinary READBACK or SCANOUT_READBACK refusal semantics.

Validate record version 2, flags bit 0 set, nonzero `present_fence`, `unwritten = 0`, backend equal to `CAPS.backend`, rectangle equal to the mode at the known scan origin, and context different from the runner's stage context. A stage picture contains changing status text and is not a case picture.
Hash canonical BGRX rows with SHA-256, forcing X to `0xff` as `suite::judged` does. Validate record fields but do not hash them. The same canonical picture has the same digest across backends.

The default `end` checkpoint runs in the case process before display-owner teardown. For helpers, a suite-library pre-drop hook observes after the return expression has judged the work and before the helper is killed or reaped. The snapshot is transported after the case reports. The child case clock and parent timeout exclude the hook's own measured duration; helper cleanup retains its existing charge. Observation has an independent 120-second wall watchdog; setup retains the same wall watchdog without checkpoint exclusions. A warm-window signal carries its exclusion baseline, so setup exclusions are not subtracted twice. The end checkpoint and its publication are excluded after the report. A checkpoint error is a separate SCREEN record and cannot change the case verdict. If it cannot observe a case binding it emits no `end`. Backends with separate device contexts exclude the runner context; GK208 reports one shared hardware channel, so its bound resource identity excludes the runner surface instead; an approved table entry without its record fails. A case can call `suite::screen_checkpoint("<name>")` after present completes. Names are lowercase hyphenated words; `end` and `intent` are reserved. Performance cases declare no named checkpoints inside measured windows. Named records travel through a `.screen` side file with integrity hashing like `.values`.

The runner emits each record through the root-only, suite-only `/dev/screen-evidence` sink immediately after the case's CASE line. That writer bypasses console, VT, kernel-log and boot-producer capture; SCREEN records do not use stdout:

```text
SUITE <id> SCREEN <category>/<case> <checkpoint> sha256=<64 hex> mode=<W>x<H> binding=<BW>x<BH>
SUITE <id> SCREEN <category>/<case> <checkpoint> sha256=<64 hex> mode=<W>x<H> binding=<BW>x<BH> crc=<8 hex> crc2=<8 hex> raster=<W>x<H> out=sor<N>
SUITE <id> SCREEN <category>/<case> <checkpoint> error=<reason-word> errno=<n> eio_reason=<n> failed_submission=<n>
SUITE <id> SCREEN <category>/<case> end unobservable reason=<device-reset|target-invalidated> errno=<n> eio_reason=<n> failed_submission=<n>
```

The CRC form is for backend 6. Endpoint failures retain the exact errno, EIO reason and invalidating submission in a `SCREENER` status occupying the first 32 bytes of the suite-only request's record buffer; ordinary GPU observation records are unchanged. Only a known end state with producer evidence of deliberate invalidation may be declared unobservable. Transfer errors remain errors. Declared records are listed as INELIGIBLE and cannot satisfy an approved digest. Serial carries no pixel payload. The suite verdict accepts SCREEN as a live, non-counting record before the guest emits it. Checkpoints default on. `scripts/boot-interactive.sh --screen-checkpoints on|off` sets `opt/breenix/screen-checkpoints`, exposed through `/proc/breenix/screen-checkpoints`. Cost is measured by one on boot and one off boot per suite at the same commit; if suite elapsed time rises by more than 10%, reuse a case's last whole-mode observation only when its resource and present fence match. Staging remains charged and reclaimed under the GPU contract. A digest that changes between runs of the same commit and target is a nondeterminism defect to diagnose and fix, with no masks or tolerance.

## Checkpoint cost evidence

Controlled ARM64 QEMU off/on captures at `a89655ad` measured GPU 226800/231850 ms (+2.227%, boots `20261101T211137Z-qemu-64661`/`20261101T211556Z-qemu-54996`) and BreenGL 285469/302411 ms (+5.935%, boots `20261101T212238Z-qemu-94645`/`20261101T212751Z-qemu-7305`); these are within the 10% budget on ARM64, while native elapsed times from different commits do not measure native checkpoint overhead.

## Phase-one capture acceptance

`scripts/screen-proof-acceptance.py --commit <measured-sha> --console <boot-dir> --gpu <boot-dir> --arm-gpu <boot-dir> --arm-breengel <boot-dir> --output <report.json>` scores retained captures without booting. The report separates the measured commit from the scorer commit. It requires distinct native console and GPU companions, console handoff-to-DONE within 330 seconds, GPU 170/170, ARM GPU 170/170 and ARM BreenGL 401/401 at the measured commit, memory reclamation, and SCREEN validation. Missing or stale ARM companions leave `proven:false`.

`docs/suites/frames/checkpoints.json` records checkpoint names, not approved pictures: the retained phase-one GPU inventory has 88 names and the BreenGL inventory has 43. The gate compares the complete inventory, so an empty approval table cannot hide missing checkpoints. It lists GT 710 startup checks by their native manifest IDs and supplying boots, counting overlapping observations once. Those checks are derived from native startup evidence; they are neither GPU CASE names nor the BreenGL selection in `gt710-step-9-1.json`. The diagnostic `core` category needs separate captures and is outside these startup checks. Reset faults must belong to the three deliberate watchdog-reset cases, with their own reset-ms and PASS records; the report retains the serial records and source attribution. A reset or reprobe refusal fails acceptance.

This phase-one gate reports unapproved checkpoints. Picture approval, the gallery, intent frames and native output CRC acceptance remain subsequent work under the contract below.

## Shared table and approval

`docs/suites/frames/screens.json` is a version 1 table shared by targets. Its unique key is `(suite, case, checkpoint)`; mode and SHA-256 are expected values. A different mode fails and does not create another entry. An entry records the intent, operator approval file, candidates digest, capture run and commit. Native rows record output, raster, output-configuration digest, CRC values, deriving run and commit. Approval files under `docs/suites/frames/approvals/` are append-only.

Only `scripts/screens/approve.py` writes the table. It accepts a verified candidates file and an operator approval exported by `vigil-cli breenix screens export`, or a coordinator relay quoting the operator's words verbatim with time and gallery identity. Agents do not author approvals; runs, scorers and launchers do not write approvals or table entries. The tool adds or replaces only approved rows whose pictured bytes hash to the candidate digest. A changed digest needs a new approval, with a delta gallery showing old and new pictures; the prior approval remains. Existing reviewed references remain unchanged and are presented for operator review.

Vigil's Breenix screen gallery presents verified captured pictures and their case, intent, checkpoint, mode, digest and boot. The operator approves or rejects there; a rejection requires a reason. Capture export verifies bytes against serial before placing candidates in Vigil. Vigil stores decisions outside the repository, and its CLI exports the approval consumed by `approve.py`.

`scripts/screen-verdict.py` scores beside the suite verdict. It fails malformed records, unknown cases, duplicate checkpoints, missing approved records, errors, digest or mode mismatches, and, when requested, missing or mismatched native CRCs or host captures. A record without an entry is listed as unapproved; milestone acceptance requires no unapproved records. Strict table validation requires an operator approval naming the exact digest and candidates identity, and an approved parent for each native row.

## Native display output

On backend 6 a checkpoint also captures the SOR and raster-generator (RG) CRCs of the active raster. Pin the latched resource and present fence, clear the notifier, arm SOR and RG through the core channel, and wait for four stable nonzero entries after the first written entry, within 250 ms. Require no overflow or display error and an unchanged binding, then disarm. The compositor CRC is retained as evidence but is not judged until characterized. If hardware does not populate the secondary output, capture RG separately and omit `crc2`.

The root-only, suite-boot-only `/proc/breenix/display-crc` read returns:

```text
backend=6 head=H out=sorN raster=WxH resource=R present_fence=F crc=XXXXXXXX crc2=XXXXXXXX entries=K status=S
```

It returns ENOSYS outside suite boots and ENODEV on backends 4 and 5. A checkpoint emits CRCs only when the read's resource and present fence match the scanout record. This interface does not add a syscall 416 operation or consume reserved record bytes.

A native CRC row is derived only from a native run where the checkpoint digest matched an operator-approved entry, with stable entries and no overflow. It is keyed by that digest and output configuration, including raster, mode, viewport, scaler methods, output and source. It judges display output after memory: it cannot admit a picture or override a digest mismatch. Mode 0 covers the 1:1 raster; mode 1 covers the scaled output, with source-pixel sensitivity characterized separately. The CRC does not establish the behavior of the link after the SOR, cable or panel.

One-shot test-control point 32, `display-output-perturb`, enables dithering for the next backend 6 checkpoint. Its native case requires the memory digest to stay unchanged while the CRC changes, then disarms and requires the approved CRC again. On ARM the point returns EINVAL.

<!-- claim-lint:ok: Proposed host-surface experiment and observation boundary in docs/gui/screen-proof.md, not an acceptance result. -->
ARM64 QEMU has no equivalent display CRC. A proof-mode host-surface capture spike may compare QEMU's displayed surface with serial during the held intent frame. If it cannot produce a valid current surface, the stated ARM boundary is the latched-binding readback and host-renderer attestation, not proof of the host window.

## Retained phase-one acceptance

Run `scripts/screen-proof-acceptance.py` with `--commit <runtime SHA>`,
`--console <complete metal console capture>`, `--gpu <complete metal GPU capture>`,
`--arm-gpu <QEMU GPU capture>`, `--arm-breengel <QEMU BreenGL capture>` and
`--output <report.json>`. Keep the measured runtime commit distinct from the
report's scorer commit when only host scoring code changes.

Use complete metal exports, including `gt710-pixels.rle` and `gt710-final.rle`,
whose lengths and hashes must match their serial anchors. Vigil's top-level
metal boot summaries omit these bulk files (`BreenixMetal.importedFiles` in
Vigil); they are insufficient inputs. When a complete export was retained under
a boot's `host-evidence` directory, select that export explicitly. Keep missing
or corrupt artifacts as failures; do not substitute serial anchors for bytes.

Attribute each intentional native reset to its pending manifest case interval,
ending at that case's passing CASE record. Require exactly one matching
`reset-ms` VALUE inside that interval. The VALUE and native FAULT have separate
serial producers, so either may arrive first within the interval; a witness
from a completed or different case does not qualify.

## Intent frame

<!-- claim-lint:ok: Normative intent-frame requirements in docs/gui/screen-proof.md, not a measured result. -->
Proof mode uses `opt/breenix/screen-proof`; it is opt-in. After a pictured case's verdict, timer and `end` checkpoint, the runner uses BGL `solid` and `nearest-scale` pipelines to present a text-free frame: background RGB (12,19,31), the judged picture centered at the largest integer scale that fits with 64 pixels of margin in each dimension, and a two-pixel white border immediately outside it. Center coordinates use floor division. The runner takes an `intent` checkpoint and holds the frame for `pace_ms` (default 1,000 ms), outside measured windows. The next stage frame then follows. This adds a screen verdict without changing the case verdict, timing or VALUE records.

The coordinator's 2026-11-02 5:30 AM EST large-picture ruling gives three layouts.
When an integer scale of at least one fits within mode width minus 64 and mode
height minus 64, use the largest such scale and the border described above.
A picture equal to the mode in both dimensions fills the screen at 1:1, without
a border. Otherwise choose the smallest integer divisor k whose k-spaced samples
fit those inner bounds. Take source coordinates 0, k, 2k and so on on both axes,
including a final partial row or column; center the resulting picture and add the
border. The `intent` digest hashes the actual displayed screen. These layouts
serve the viewer and do not change the picture judged by the case.

<!-- claim-lint:ok: Normative launch interface in scripts/boot-interactive.sh and renderer in libs/libbreenix/src/suite_intent.rs. -->
On ARM64 QEMU, `scripts/boot-interactive.sh --mode suite --suite gpu|breengel
--gpu-gl --screen-proof [--pace MS]` selects this view through fw_cfg. Intent
rendering transports the verified snapshot unchanged to a separate GPU texture;
GPU solid and nearest-scale draws compose it. Its context is released after the
checkpoint and hold, before the next case starts.
