# Screen proof contract <!-- claim-lint:ok: Normative contract specified in docs/gui/screen-proof.md, not a measured result. -->

<!-- claim-lint:ok: Normative contract specified in docs/gui/screen-proof.md, not a measured result. -->
Screen proof adds checkpoints and a separate verdict alongside the GPU and BreenGL suite verdicts. Existing cases, expected values, thresholds and reviewed references remain unchanged. This is the `screens` milestone of E22 on both architecture tabs and a prerequisite for E22 `frames`. G29 adopts the shared picture table and the derived native display-output evidence described below. This document specifies the intended contract; implementation and acceptance belong to subsequent units.

## Observation and records

<!-- claim-lint:ok: Observation contract here; scan origins from kernel/src/gpu/virgl/mod.rs set_scanout, kernel/src/gpu/svga/mod.rs set_scanout and kernel/src/gpu/gk208/modeset.rs Viewport::methods. -->
A checkpoint observes exactly the mode-sized rectangle scanned out from the bound resource. A larger binding is valid; a smaller binding or unknown scan origin is an error. All current backends program scan origin (0,0): VirGL SET_SCANOUT, SVGA screen binding and GK208 viewport-in. The record's rectangle must match that origin and `CURRENT_MODE`; target dimensions describe the binding and must contain the rectangle. The SCREEN line records both mode and binding dimensions.

<!-- claim-lint:ok: Endpoint semantics specified here and implemented by kernel/src/gpu/mod.rs observe_screen and read_back. -->
Checkpoint readback uses the root-only, suite-only `/dev/screen-evidence` binary request: exactly 32 bytes, `SCREENRB` followed by little-endian u64 pixel pointer, buffer length and record pointer. The GPU core pins the binding, observes the visible mode rectangle with its ordinary poison/transfer checks, and writes canonical pixels plus record v2. This pixel-only endpoint carries no composition provenance (flag bit 1, composite serial and window entries stay zero), so the number of windows cannot truncate its record. Ordinary READBACK and SCANOUT_READBACK retain their complete-provenance requirement and reason-11 refusal above 16 windows, with untouched outputs. No syscall operation or reserved record field changes.

Validate record version 2, flags bit 0 set, nonzero `present_fence`, `unwritten = 0`, backend equal to `CAPS.backend`, rectangle equal to the mode at the known scan origin, and context different from the runner's stage context. A stage picture contains changing status text and is not a case picture.
Hash canonical BGRX rows with SHA-256, forcing X to `0xff` as `suite::judged` does. Validate record fields but do not hash them. The same canonical picture has the same digest across backends.

The default `end` checkpoint runs in the case process before display-owner teardown. For helpers, a suite-library pre-drop hook observes after the return expression has judged the work and before the helper is killed or reaped. The snapshot is transported after the case reports. The child case clock and parent timeout exclude the hook's own measured duration; helper cleanup retains its existing charge. A checkpoint error is a separate SCREEN record and cannot change the case verdict. If it cannot observe a binding outside the runner's context it emits no `end`; an approved table entry without its record fails. A case can call `suite::screen_checkpoint("<name>")` after present completes. Names are lowercase hyphenated words; `end` and `intent` are reserved. Performance cases declare no named checkpoints inside measured windows. Named records travel through a `.screen` side file with integrity hashing like `.values`.

The runner emits each record through the root-only, suite-only `/dev/screen-evidence` sink immediately after the case's CASE line. That writer bypasses console, VT, kernel-log and boot-producer capture; SCREEN records do not use stdout:

```text
SUITE <id> SCREEN <category>/<case> <checkpoint> sha256=<64 hex> mode=<W>x<H> binding=<BW>x<BH>
SUITE <id> SCREEN <category>/<case> <checkpoint> sha256=<64 hex> mode=<W>x<H> binding=<BW>x<BH> crc=<8 hex> crc2=<8 hex> raster=<W>x<H> out=sor<N>
SUITE <id> SCREEN <category>/<case> <checkpoint> error=<reason-word>
```

The second form is for backend 6. Serial carries no pixel payload. The suite verdict accepts SCREEN as a live, non-counting record before the guest emits it. Checkpoints default on. `scripts/boot-interactive.sh --screen-checkpoints on|off` sets `opt/breenix/screen-checkpoints`, exposed through `/proc/breenix/screen-checkpoints`. Cost is measured by one on boot and one off boot per suite at the same commit; if suite elapsed time rises by more than 10%, reuse a case's last whole-mode observation only when its resource and present fence match. Staging remains charged and reclaimed under the GPU contract. A digest that changes between runs of the same commit and target is a nondeterminism defect to diagnose and fix, with no masks or tolerance.

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

## Intent frame

<!-- claim-lint:ok: Normative intent-frame requirements in docs/gui/screen-proof.md, not a measured result. -->
Proof mode uses `opt/breenix/screen-proof`; it is opt-in. After a pictured case's verdict, timer and `end` checkpoint, the runner uses BGL `solid` and `nearest-scale` pipelines to present a text-free frame: background RGB (12,19,31), the judged picture centered at the largest integer scale that fits with 64 pixels of margin in each dimension, and a two-pixel white border immediately outside it. Center coordinates use floor division. The runner takes an `intent` checkpoint and holds the frame for `pace_ms` (default 1,000 ms), outside measured windows. The next stage frame then follows. This adds a screen verdict without changing the case verdict, timing or VALUE records.
