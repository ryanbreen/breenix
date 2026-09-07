# Parallels capture path — diagnosis and fix, 2026-09-07

Branch `tools/parallels-capture-fix`, off `origin/main` at `68f73db5`. Issue
[#917](https://github.com/ryanbreen/breenix/issues/917).

## Task

`./run.sh --parallels --test N`'s screenshot step was scoring the capture
mechanism, not the kernel: `scripts/parallels/screenshot-vm.sh`'s window
lookup failed in 7 of the 7 lifecycle runs in
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/`
(`ERROR: No Parallels window found matching <vm>` in each run's
`stdout.log` — see e.g.
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-1/stdout.log:508`),
and `run.sh`'s fallback (a bare `prlctl capture ... 2>/dev/null`) then
reported `Screenshot: /tmp/breenix-screenshot.png` regardless of what it
actually captured — five of those seven screenshots are 3674-byte
solid-black PNGs (claim-lint:ok: 5/7, counted directly from the seven
`screenshot.png` files under that evidence directory).

## Diagnosis

### (a) Why `prlctl capture` fails, and why it sometimes doesn't

It doesn't reliably fail. Direct testing (evidence:
`evidence/prlctl-capture-failure-modes.txt`) shows `prlctl capture` returns
exit 0 with a real, non-synthetic frame whenever the named VM exists and is
running — including the black frames: `prlctl capture`'s own stdout
literally prints `Capture the VM screen... / The VM screen is stored in
/tmp/breenix-screenshot.png.`, which is `prlctl`'s own message, not
anything `run.sh` or `screenshot-vm.sh` writes. The 3674-byte black PNGs in
the sweep evidence are genuine captures of a real early-boot state (the
guest compositor had not rendered anything to the VirtIO GPU surface yet),
not a stub.

`prlctl capture` DOES fail, with exit code 255 in both tested cases below
and an explanatory stderr message, when there is no live VM to connect to:

```
$ prlctl capture does-not-exist-vm-xyz --file /tmp/x.png
Failed to get VM config: The virtual machine could not be found. The
virtual machine is not registered in the virtual machine directory on your
Mac.
exit=255

$ prlctl capture linux-probe --file /tmp/x.png   # linux-probe is 'suspended'
Capture the VM screen...
PrlDevDisplay_ConnectToVm: PrlJob_Wait: PRL_ERR_IO_STOPPED
exit=255
```

(preserved: `evidence/prlctl-capture-failure-modes.txt`). Neither of the old
scripts captured this exit code or stderr text anywhere — both piped
`prlctl capture`'s stderr straight to `/dev/null`.

### (b) Why the window lookup fails: it's not a flake

`screenshot-vm.sh` (and the pre-fix `capture-display.sh`) find a Parallels
window by matching the VM's name as a substring of `kCGWindowName`. Direct
inspection of `Quartz.CGWindowListCopyWindowInfo`, both with a Breenix VM
running (captured live during the diagnosis boot, VM `breenix-1788760315`)
and with **no** Breenix VM running at all
(`evidence/windowlist-no-vm.txt`), shows every window owned by "Parallels
Desktop" reports `kCGWindowName == ''`:

```
TOTAL Parallels-owned windows: 12
Windows with non-empty kCGWindowName: 0
```

Parallels sets no per-VM window title in the 12/12 Parallels-owned windows
inventoried both with a Breenix VM running and with none running
(`evidence/windowlist-no-vm.txt`: `TOTAL Parallels-owned windows: 12`,
`Windows with non-empty kCGWindowName: 0`) when the VM is started via
`prlctl start` with no GUI open — this harness's normal mode (`ps aux`
during a live boot shows `prl_vm_app --vm-name <VM>` running as the actual
per-VM backend process; the "Parallels Desktop" GUI application itself,
which is what would normally title its windows, is not running). A
substring match against an always-empty string cannot succeed by
construction. This is consistent with why all 7 of 7 historical runs
printed the identical `ERROR: No Parallels window found matching '<vm>'`.

### (c) Where the 3674-byte black PNG comes from

`prlctl capture` itself, called too early. It is a real 1280×960 8-bit RGB
PNG (`file` confirms `PNG image data, 1280 x 960, 8-bit/color RGB,
non-interlaced`); a `PIL` `Counter` over its pixel data reports exactly one
distinct color, `(0, 0, 0)`, covering the full 1,228,800-pixel frame — a
genuine capture of the VM's display surface before the VirtIO GPU
compositor has rendered its first frame, not a placeholder written by
either script. Neither old script's source contains any code path that
writes a synthetic image (`git show 68f73db5:scripts/parallels/screenshot-vm.sh`
and `git show 68f73db5:run.sh` at this branch's base commit); the only
write path in `run.sh`'s old fallback was the same `prlctl capture ...
--file "$SCREENSHOT"` line whose real output this is.

### One more thing this diagnosis had to work around

The first attempt at proof cycle 1 (its raw files were not preserved — this
paragraph is from direct observation during the diagnosis session, not
from a committed artifact) captured a real but content-free frame: a solid
`(100, 149, 237)` (cornflower-blue) 1280×960 image — `f24-render-verdict.sh`
reported `blue_baseline=True`, `VERDICT=FAIL` on it. That boot's serial log
contained zero lines matching `bsshd`/`bounce`/`composite-submit`, and
`strings` on the kernel binary this worktree had inherited by copying
`target/aarch64-breenix-kernel/release/kernel-aarch64` from the `main`
checkout showed `[BOOT_TESTS:START]` and `BOOT_TEST_CPU_AFFINITY` symbols:
that binary's last build on `main` had used `--features boot_tests`, so it
was running the boot-test framework instead of launching the normal
desktop services. Rebuilding the plain production kernel in this worktree
(`cargo build --release --target aarch64-breenix-kernel.json -Z
build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel
--bin kernel-aarch64`, no `--features`) produced a binary with zero
`BOOT_TESTS` strings (re-checked with the same `strings | grep` after the
rebuild), and the very next boot's capture was a real 547-distinct-color
desktop frame (`evidence/cycle1/stdout.log`: `Attempt 1: method=prlctl
size=1280x960 dominant=0,0,0 distinct=547`). This is not a defect in the
capture path — `capture-display.sh` reported `method=prlctl:reason=ok`
for the cornflower-blue frame too, since it *was* a real, valid capture —
and is disclosed here only because it explains why the proof runs below
needed a rebuilt kernel binary, not because it's part of this issue's fix.

## Fix

- **`scripts/parallels/capture-display.sh`** (rewritten): drops
  title-substring window matching (per (b), a match against an always-empty
  string cannot succeed) in favor of matching a candidate window by the PID
  of the actual `prl_vm_app --vm-name <VM>` backend process, and logs the
  full `CGWindowList` inventory (owner, title, pid, layer, size) to stderr
  on each attempt rather than a single opaque "not found" line. Captures
  `prlctl capture`'s real exit code and stderr (per (a)) instead of
  discarding both. Still retries past an early black warmup frame on the
  existing schedule. Writes to `OUTPUT` from exactly one place — the `cp
  "$candidate" "$OUTPUT"` after a captured frame has been verified
  non-black — checked by
  `capture_display_writes_output_exactly_once_and_only_on_a_verified_frame`
  in `tests/parallels_capture_structure.rs` (claim-lint:ok: pinned by that
  test, 7/7 passing per the "Structural test" section below). Emits
  `[PARALLELS_CAPTURE:method=<prlctl|window|none>:reason=<...>]` on stdout
  on both the success and the terminal-failure path (see the cycle 1/2
  transcripts under "Verification runs" below for both shapes in real
  output).
- **`run.sh`**: the test-mode screenshot step now calls
  `capture-display.sh` — previously dead code from `run.sh`'s own
  perspective, which called `screenshot-vm.sh` and a bare inline `prlctl
  capture` fallback instead — and reports `capture=<method>` or
  `capture=none` explicitly instead of the old unconditional `Screenshot:
  ...` line, checked by
  `run_sh_has_no_silent_capture_fallback_and_calls_capture_display` in
  `tests/parallels_capture_structure.rs` (claim-lint:ok: same test file,
  7/7 passing).
- **`scripts/f24-render-verdict.sh`**: refuses with a distinct
  `CAPTURE_MISSING` verdict (exit 2) when the PNG path is absent, or when
  the image is a solid single color whose RGB channels are each `<= 8`
  (checked over the whole frame, not a byte-identical hash) — before
  reaching the existing render-quality checks, so `VERDICT=FAIL` now means
  only "captured the desktop and the render-quality checks below scored it
  low," distinct from `CAPTURE_MISSING` meaning "no desktop capture to
  score." Checked by `f24_render_verdict_rejects_missing_capture` and
  `f24_render_verdict_rejects_solid_black_capture` (claim-lint:ok: same
  test file, 7/7 passing).
- **`scripts/parallels/screenshot-vm.sh`**: turned into a one-line
  compatibility wrapper (`exec capture-display.sh "$@"`) with a header
  explaining why, since it's no longer on `run.sh`'s path but a direct
  invocation should still produce a real screenshot instead of the old
  window-title match this doc's (b) section describes.

## Structural test

`tests/parallels_capture_structure.rs`, 7/7 tests passing:

```
running 7 tests
test run_sh_has_no_silent_capture_fallback_and_calls_capture_display ... ok
test run_sh_silent_fallback_check_is_not_vacuous ... ok
test capture_display_writes_output_exactly_once_and_only_on_a_verified_frame ... ok
test capture_display_output_write_check_is_not_vacuous ... ok
test f24_render_verdict_rejects_missing_capture ... ok
test f24_render_verdict_rejects_solid_black_capture ... ok
test f24_render_verdict_still_passes_a_real_capture ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.66s
```

(re-run independently after Codex's own run reported the same 7/7, both
with `BREENIX_RUST_FORK_LIBRARY` set — required because `kernel/build.rs`
builds the userspace test corpus even for this host-side-only test target).

The mutation this suite is built to catch —
`capture_display_output_write_check_is_not_vacuous` — reinstates the
historical failure shape (an unconditional second `cp "$candidate"
"$OUTPUT"` spliced in near the terminal failure path, simulating "just
write something so `run.sh` has a file to point at") into
`capture-display.sh`'s own real text and asserts the write-count predicate
goes from 1 match to more than 1. `f24_render_verdict_still_passes_a_real_capture`
is the paired regression guard: a real, richly-colored synthetic fixture
must still get `VERDICT=PASS` through the new `CAPTURE_MISSING` preflight,
proving that preflight doesn't also swallow a legitimate pass.

## Verification runs

### Five consecutive captures on one live VM at desktop

VM `breenix-1788762666` (production, no-`boot_tests` kernel, per the
"one more thing" note above), captured five times in a row with
`capture-display.sh` directly, each checked with `f24-render-verdict.sh`.
5/5: `distinct > 1`, coherent-region `VERDICT=PASS`, method
`prlctl:reason=ok` (see the table and the per-capture `.verdict.log` files
below). Preserved: `evidence/five-captures/capture-{1..5}.png` +
`.stdout.log` (verdict line) + `.verdict.log` (render verdict) each.

| # | distinct | dom_frac | big_buckets | VERDICT |
|---|---|---|---|---|
| 1 | 1936 | 0.090 | 12 | PASS |
| 2 | 2002 | 0.090 | 12 | PASS |
| 3 | 2002 | 0.090 | 12 | PASS |
| 4 | 1888 | 0.170 | 10 | PASS |
| 5 | 1341 | 0.840 | 3 | PASS |

5/5 files have distinct md5s and sizes ranging 20503–30322 bytes
(`evidence/five-captures/md5sums.txt`), consistent with five separate
captures rather than one image copied five times.

### Two full `./run.sh --parallels --test 120` cycles, end to end

Cycle 1 — VM `breenix-1788762666`:

```
=== Screenshot ===
Attempt 1: waiting 5s before capture for VM 'breenix-1788762666'
prlctl capture: exit=0
Attempt 1: method=prlctl size=1280x960 dominant=0,0,0 distinct=547
Solid-red baseline comparison: different
[PARALLELS_CAPTURE:method=prlctl:reason=ok]
/tmp/breenix-screenshot.png
Screenshot: /tmp/breenix-screenshot.png (capture=prlctl)
```

`f24-render-verdict.sh` on the resulting `evidence/cycle1/screenshot.png`:
`distinct=378 dominant=(0, 0, 0) dom_frac=0.8663 big_color_buckets=3
blue_baseline=False red_baseline=False ... VERDICT=PASS`.

Cycle 2 — VM `breenix-1788762977`:

```
=== Screenshot ===
Attempt 1: waiting 5s before capture for VM 'breenix-1788762977'
prlctl capture: exit=0
Attempt 1: method=prlctl size=1280x960 dominant=10,10,25 distinct=2158
Solid-red baseline comparison: different
[PARALLELS_CAPTURE:method=prlctl:reason=ok]
/tmp/breenix-screenshot.png
Screenshot: /tmp/breenix-screenshot.png (capture=prlctl)
```

`f24-render-verdict.sh` on `evidence/cycle2/screenshot.png`:
`distinct=2002 dominant=(10, 10, 25) dom_frac=0.0896 big_color_buckets=12
blue_baseline=False red_baseline=False ... VERDICT=PASS`.

Both cycles: `run.sh` exit 0, `method=prlctl`, `VERDICT=PASS`. Preserved:
`evidence/cycle{1,2}/{stdout.log,serial.log,screenshot.png}`.

Both VMs were `prlctl stop --kill`'d, polled to `stopped`, then `prlctl
delete`'d after their evidence was copied out. A full `prlctl list` after
cleanup shows exactly one remaining VM, `linux-probe suspended` — the
same state it was in before this round started, so no `prlctl
stop`/`delete` command naming `linux-probe` was issued during this round.

## claim-lint

```
claim-lint: python3 scripts/claim-lint.py                                    -> exit 0
claim-lint: python3 scripts/claim-lint.py --files docs/planning/green-program/gui/PARALLELS-CAPTURE-2026-09-07.md -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg /tmp/pcap-commit-msg.txt -> exit 0
```

## Not claimed

- Not claimed: that the blue-idle-desktop / boot_tests-binary confusion
  described in "one more thing this diagnosis had to work around" is
  itself fixed by this PR. It isn't — it was an artifact of this worktree
  inheriting a stale binary from a differently-configured `main` build,
  not a defect in any file this PR touches, and it disappeared the moment
  a plain production kernel was rebuilt. It is recorded here only so a
  future reader hitting the same cornflower-blue capture in a fresh
  worktree does not re-diagnose it as a capture-path bug.
- Not claimed: that `scripts/parallels/start-and-screenshot.sh` (a
  separate, `run.sh`-independent tool not touched by this PR) shares or
  doesn't share the window-title-matching defect described in (b). It
  wasn't inspected as part of this round; it isn't on `run.sh`'s path.
- Not claimed: a fix for the pre-existing, separately-tracked aarch64
  Parallels boot-stall class (#906) or the render-quality thresholds in
  `f24-render-verdict.sh` (unchanged by this PR except for the new
  `CAPTURE_MISSING` preflight, which runs strictly before them).
- Not claimed: that `prlctl capture`'s exit-code/stderr contract is stable
  across Parallels Desktop versions — this diagnosis was run against
  `prlctl version 26.4.1 (57516)`, the version installed on this machine.
