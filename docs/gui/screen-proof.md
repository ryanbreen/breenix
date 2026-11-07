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
SUITE <id> SCREEN <category>/<case> <checkpoint> sha256=<64 hex> mode=<W>x<H> binding=<BW>x<BH> crc=<8 hex> crc2=<8 hex|none> raster=<W>x<H> out=sor<N>
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

## Gallery capture acceptance

Pass `--phase gallery` to the retained-capture gate for phase two, together with
`--gpu-candidates <manifest.json>` and `--breengel-candidates <manifest.json>`.
This mode retains the phase-one boot, commit, console time, suite, memory and
native startup checks. In the ARM proof captures its checkpoint inventory adds an
`intent` for each CASE carrying a judged-picture digest; the native GPU capture is
a production companion with end checkpoints only until native proof mode (U19). The two candidates manifests must contain the
corresponding ARM boot's SCREEN pictures, with matching mode, binding, digest,
boot and runtime commit; the approval importer's existing validator checks the
manifest identity and PNG pixels. Separate ARM boot IDs are still required.
The report identifies its phase and retains exit status, failed CASE lines,
DONE lines and memory records even when boot eligibility fails.

This checks capture evidence for review; it does not approve candidates,
supersede operator decisions or establish the later approved-table/native-CRC
milestone. A failed boot or missing companion leaves `proven:false`, covered by
the failed-boot regression in `tests/screen_acceptance_test.py`.

## Shared table and approval

`docs/suites/frames/screens.json` is a version 1 table shared by targets. Its unique key is `(suite, case, checkpoint)`; mode and SHA-256 are expected values. A different mode fails and does not create another entry. An entry records the intent, operator approval file, candidates digest, capture run and commit. Native rows record output, raster, output-configuration digest, CRC values, deriving run and commit. Approval files under `docs/suites/frames/approvals/` are append-only.

Only `scripts/screens/approve.py` writes the table. It accepts a verified candidates file and an operator approval exported by `vigil-cli breenix screens export`, or a coordinator relay quoting the operator's words verbatim with time and gallery identity. Agents do not author approvals; runs, scorers and launchers do not write approvals or table entries. The tool adds or replaces only approved rows whose pictured bytes hash to the candidate digest. A changed digest needs a new approval, with a delta gallery showing old and new pictures; the prior approval remains. Existing reviewed references remain unchanged and are presented for operator review.

Vigil's Breenix screen gallery presents verified captured pictures and their case, intent, checkpoint, mode, digest and boot. The operator approves or rejects there; a rejection requires a reason. Capture export verifies bytes against serial before placing candidates in Vigil. Vigil stores decisions outside the repository, and its CLI exports the approval consumed by `approve.py`.

`scripts/screen-verdict.py` scores beside the suite verdict. It fails malformed records, unknown cases, duplicate checkpoints, missing approved records, errors, digest or mode mismatches, and, when requested, missing or mismatched native CRCs or host captures. A record without an entry is listed as unapproved; milestone acceptance requires no unapproved records. Strict table validation requires an operator approval naming the exact digest and candidates identity, and an approved parent for each native row.

## Native display output

On backend 6 a native row judges the SOR output CRC alone (coordinator ruling U13,
E22 `rulings.md`, 2026-11-07). The SOR is the output resource closest to the
connector. Captures 20261107T082545Z, 20261107T084839Z, 20261107T090458Z and
20261107T092218Z supplied SOR words but no RG words, including primary-RG,
reversed-pair, wide/narrow and snooze-enabled observations. No RG probe runs in
the production checkpoint path.

Pin the latched resource and present fence, clear the notifier and arm the
core-channel CRC context. Acquire at least five written primary entries, disarm,
wait for DONE, then re-read the completed notifier. Discard its first entry;
require K=4 or more consecutive stable nonzero SOR entries within 250 ms.
Overflow, display errors, an unstable completed tail, a deadline, a moved binding
or changed output methods refuse the observation.

<!-- claim-lint:ok: U13 contract, exercised by tests/screen_sor_crc_test.py and tests/fixtures/gt710_display_crc.rs. -->
The SCREEN record explicitly says `crc2=none`. `SCRCRC01` v2 adds a little-endian
u32 reason tag at byte 116 (1 = `gk208-rg-no-words-u13`), extending the fixed
header from 116 to 120 bytes. The unchanged three-word sample layout retains a
reserved secondary word that must be 0; it is not a numeric CRC. The exporter
writes `secondary_absent: "gk208-rg-no-words-u13"` and sample `crc2: null`.
Unknown reasons and contradictory secondary words fail. Legacy v1 records
remain dual-source records and must supply nonzero, stable secondary CRCs.
Only backend 6 admits the explicit absence; backends 4 and 5 retain their
readback/host-renderer contract. Native approved rows carry `backend: 6`,
`crc2: "none"` and the same `secondary_absent` reason. Numeric zero, garbage,
a serial/sample mismatch or an unmarked absence is refused.

The completion sequence follows NVIDIA's MIT
[Display-CRC notes](https://github.com/NVIDIA/open-gpu-doc/blob/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/Display-CRC/display-crc.txt#L69-L78).
For a static pinned frame, CRC reporting during snooze is enabled using the
same document's NV907D `CRC_DURING_SNOOZE` bit 5 (lines 314-319).

The root-only, suite-boot-only `/proc/breenix/display-crc` read returns:

```text
backend=6 head=H out=sorN raster=WxH resource=R present_fence=F crc=XXXXXXXX crc2=none entries=K status=S
```

It returns ENOSYS outside suite boots and ENODEV on backends 4 and 5. A checkpoint emits CRCs only when the read's resource and present fence match the scanout record. This interface does not add a syscall 416 operation or consume reserved record bytes.

A native CRC row is derived only from a native run where the checkpoint digest matched an operator-approved entry, with stable entries and no overflow. It is keyed by that digest and output configuration, including raster, mode, viewport, scaler methods, output and source. It judges display output after memory: it cannot admit a picture or override a digest mismatch. Mode 0 covers the 1:1 raster; mode 1 covers the scaled output, with source-pixel sensitivity characterized separately. The CRC does not establish the behavior of the link after the SOR, cable or panel.

One-shot test-control point 35, `display-output-perturb`, enables dithering for the next backend 6 checkpoint. Its native case requires the memory digest to stay unchanged while the SOR CRC changes in phase 3, then disarms and requires the approved
SOR CRC again in phase 4. A secondary-only change cannot satisfy phase 3. On ARM the point returns EINVAL.

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
Proof mode uses `opt/breenix/screen-proof`; it is opt-in. After a pictured case's verdict, timer and `end` checkpoint, the runner uses BGL `solid` and `nearest-scale` pipelines to present a text-free frame: a neutral checkerboard background of 16×16-pixel squares alternating #5A5A5A and #7A7A7A from the frame origin (operator, 2026-11-02 noon), the judged picture centered at the largest integer scale that fits with 64 pixels of margin in each dimension, and a two-pixel white border immediately outside it. Center coordinates use floor division. The runner takes an `intent` checkpoint and holds the frame for `pace_ms` (default 1,000 ms), outside measured windows. The next stage frame then follows. This adds a screen verdict without changing the case verdict, timing or VALUE records.

Under coordinator ruling U11 (E22, #2678), oversized intent pictures are lossless.
Keep the existing integer enlargement when it fits the 64-pixel margin. A picture
equal to the mode fills it at scale 1 without a border. Otherwise preserve scale 1,
using the space inside a complete two-pixel border and folding source cells into
bands. Comparison panels fold on their fixture column and row boundaries, retaining
labels and padding; wide strips retain both rows. Decimation is forbidden.

<!-- claim-lint:ok: U11 normative layout and count contract; tests/intent_inventory_test.py checks inventory and page counts. -->
If folding still cannot fit, use the fewest pages. Pages are consecutive checkpoints
`intent-p1`, `intent-p2`, and so on, each held for `pace_ms` at scale 1 with its own
border and digest. Page cuts preserve fixture cells where present, otherwise whole
source rows. Their source union is the judged picture, with a hit count of one per
pixel. Page approvals are separate; an unpaged approval does not approve a page.
`scripts/metal/intent-pictures.tsv` lists the judged sizes, mode and fixture cuts;
`scripts/metal/intent_manifest.py` derives host page inventories, intent counts and
proof holds. The guest includes that same manifest. Layout changes do not change
the judged bytes, case verdict, checks or thresholds.

<!-- claim-lint:ok: Normative launch interface in scripts/boot-interactive.sh and renderer in libs/libbreenix/src/suite_intent.rs. -->
On ARM64 QEMU, `scripts/boot-interactive.sh --mode suite --suite gpu|breengel
--gpu-gl --screen-proof [--pace MS]` selects this view through fw_cfg. Intent
rendering transports the verified snapshot unchanged to a separate GPU texture;
GPU solid and nearest-scale draws compose it. Its context is released after the
checkpoint and hold, before the next case starts.

## Repeated native picture parts

An intent-enabled boot can send `X-Breenix-Picture-Reference: 1` on the ordered
native `/pictures/N` route. Its digest-checked body is
`PICTURE_REF v=1 source=M bytes=B sha256=H` followed by a newline. M must be
an earlier completed part in the same suite and boot. The receiver checks that
part's receipt, size and SHA-256 again before publishing the identical payload
at N. Raw and RLE image formats and the first pictured case per category stay
unchanged. The normal picture anchors still bind the reconstructed full bytes.

The 64 MiB upload budget counts literal request bodies and reference descriptors.
Receipts retain canonical `bytes` and `sha256`; a reference additionally records
`wireBytes`, `wireSha256` and `reference`. Reconstructed native picture payloads
have their own unchanged 64 MiB ceiling. Missing, corrupt, foreign, forward,
unsealed and mode-disabled references are refused; no retry accepts weaker evidence.
