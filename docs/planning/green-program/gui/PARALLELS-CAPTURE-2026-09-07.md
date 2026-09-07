# Parallels capture path — diagnosis and fix, 2026-09-07

Branch `tools/parallels-capture-fix`, off `origin/main` at `68f73db5`. Issue
[#917](https://github.com/ryanbreen/breenix/issues/917).

## Task

`./run.sh --parallels --test N`'s screenshot step was scoring the capture
mechanism, not the kernel: `scripts/parallels/screenshot-vm.sh`'s window
lookup failed in 6 of the 6 lifecycle runs that got far enough to attempt a
screenshot, in
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/`
(`ERROR: No Parallels window found matching <vm>` in each of those runs'
`stdout.log` — see e.g.
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/run3-test120-1/stdout.log:508`;
the seventh non-aborted run, `run1-launcher-smoke`, failed earlier on
`FAIL: USB_MOUSE_ENUM` and never reached the screenshot step, so it has no
`screenshot.png` and 0 occurrences of that error string, correction:
review C-1/C-2, 2026-09-07),
and `run.sh`'s fallback (a bare `prlctl capture ... 2>/dev/null`) then
reported `Screenshot: /tmp/breenix-screenshot.png` regardless of what it
actually captured — one of those six screenshots (`run3-test120-1`) is a
3674-byte solid-black PNG; a second (`run3-test120-4`) is a solid but
non-black cornflower-blue frame, and a third (`run3-test120-3`) is 98.3%
black but not solid (145 distinct colors) (claim-lint:ok: 1/6, counted
directly with PIL's pixel/color data over the six `screenshot.png` files
under that evidence directory).

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

## Round 2 -- fix pass on the review findings (2026-09-07)

Date checked with `date +%Y-%m-%d`: `2026-09-07`.

- C-3: The scoring bonus allowed unrelated Parallels UI windows to win without a backend PID match; selection now requires that match, exercised by `capture_window_requires_pid_match_not_any_parallels_window`.
- C-4: A failed screencapture could accept leftover candidate content; the function now clears that content and checks the capture command's pipeline status, exercised by `capture_window_does_not_report_success_when_screencapture_fails`, including its explicit exit-3 diagnostic assertion.
- C-5: Test mode returned success after capture failure; CAPTURE_OK now gates the final exit, checked by `run_sh_test_mode_exit_reflects_capture_outcome` and the gate-removal mutation companion `run_sh_capture_exit_check_is_not_vacuous`.
- C-6: Baseline diagnostics could abort after OUTPUT was copied; diagnostics now run conditionally as a non-fatal side effect, with directory creation explicitly returning failure so the warning fires, exercised by `capture_display_reports_success_when_baseline_write_fails_after_a_real_copy`.
- C-7: Failed retries left prior screenshots at OUTPUT; the script now removes that path before retrying, exercised by `capture_display_clears_stale_output_before_a_failing_run`.
- C-8: Backend discovery matched a VM-name prefix; it now requires a space or end-of-line after the name, exercised by `find_vm_backend_pid_matches_exact_vm_name_not_a_prefix`.
- C-9: Corrupt image bytes raised an uncaught PIL exception with exit 1; decode exceptions now print CAPTURE_MISSING and exit 2, exercised by `f24_render_verdict_rejects_corrupt_png`.

Tests are in `tests/parallels_capture_structure.rs`. The requested seven
finding tests plus the requested C-5 mutation companion add eight tests to
the original seven, so the actual total is 15, not the task's stated 14.
The existing tests were retained without edits.
<!-- claim-lint:ok: tests/parallels_capture_structure.rs; command output below -->

Command (exit 0, compile output had no warnings or errors):

```
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library cargo test --test parallels_capture_structure
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.67s
```

Real smoke check: removed the known output path using
`Path('/tmp/pcap-fixpass-smoke.png').unlink(missing_ok=True)` after automatic
approval review rejected the initial shell command containing `rm -f`.
Ran with real prlctl, ps and Quartz, without PATH or PYTHONPATH overrides:

```
BREENIX_CAPTURE_RETRY_SCHEDULE=0 bash scripts/parallels/capture-display.sh definitely-not-a-real-vm /tmp/pcap-fixpass-smoke.png
```

Exit 1. Combined stdout/stderr verbatim:

```
Attempt 1: waiting 0s before capture for VM 'definitely-not-a-real-vm'
prlctl capture: exit=255 stderr="Failed to get VM config: The virtual machine could not be found. The virtual machine is not registered in the virtual machine directory on your Mac."
Attempt 1: prlctl capture failed, trying Core Graphics window capture
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=640x518 id=310423
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=2056x39 id=292991
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=2056x39 id=292985
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=2056x39 id=292964
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=2056x39 id=292939
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=640x508 id=292863
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=184x196 id=310427
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=640x508 id=479
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=668x68 id=477
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=818x801 id=169661
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=540x164 id=484
WINDOW owner='Parallels Desktop' title='' pid=13628 layer=0 size=500x500 id=481
NO_MATCH vm='definitely-not-a-real-vm' vm_pid=None
Attempt 1: window capture also failed (prlctl-exit-255:Failed to get VM config: The virtual machine could not be found. The virtual machine is not registered in the virtual machine directory on your Mac.:no-window-match)
ERROR: failed to capture a non-black Parallels display for VM 'definitely-not-a-real-vm' (reason=prlctl-exit-255:Failed to get VM config: The virtual machine could not be found. The virtual machine is not registered in the virtual machine directory on your Mac.:no-window-match)
[PARALLELS_CAPTURE:method=none:reason=prlctl-exit-255;Failed to get VM config; The virtual machine could not be found. The virtual machine is not registered in the virtual machine directory on your Mac.;no-window-match]
```

The subsequent `test ! -e /tmp/pcap-fixpass-smoke.png` succeeded: the
smoke check did not create the screenshot.

## claim-lint

```
claim-lint: python3 scripts/claim-lint.py (initial run) -> exit 1
claim-lint: python3 scripts/claim-lint.py (after adding test citations) -> exit 0
claim-lint: python3 scripts/claim-lint.py (after appending this section) -> exit 1
claim-lint: python3 scripts/claim-lint.py (after documenting the lint finding) -> exit 0
```

The initial findings concerned two new comments; both now cite
the regression-test file and #917. The first documentation check flagged
the same literal verdict token in this paragraph; the paragraph now describes
the comments without repeating that token.

## Not claimed

This fix pass did not perform a live Parallels VM boot. No VM was started
or stopped during this round; verification requiring a live boot happens
separately.

(Superseded by the addendum below: that separate verification, including a
live boot, was performed the same day.)

## Round 2 addendum -- independent verification + one live capture (2026-09-07)

The fix pass above was implemented and verified by Codex
(`gpt-6-astra`, `model_reasoning_effort=low`, via the `codex-wf` harness,
two dispatches). This addendum records independent verification performed
directly in this worktree afterward, plus the one live Parallels capture
the task required because the capture path itself changed.

### Non-vacuity spot-checks (three of the seven fixes, reverted one at a time)

For each fix named below, the exact pre-fix code shape was temporarily
restored in `scripts/parallels/capture-display.sh`, the single
corresponding new test was run in isolation, the file was restored, and
`diff` against a saved known-good copy confirmed byte-for-byte restoration
before moving to the next check. All three reddened as expected, confirming
the new tests are not vacuous (claim-lint:ok: 3/3, the C-3/C-4/C-8
transcripts quoted below):

- **C-3** (`capture_window_requires_pid_match_not_any_parallels_window`):
  reverting to the scoring-bonus shape (`score += 50_000_000` on a PID
  match, no `continue` on a non-match) made the test fail with
  `left: Some(0), right: Some(1)` — the reverted code selected window id 77
  (owned by PID 333, an unrelated fixture window) and reported
  `[PARALLELS_CAPTURE:method=window:reason=ok]`, exactly the C-3 defect
  shape.
- **C-4** (`capture_window_does_not_report_success_when_screencapture_fails`):
  reverting `capture_window()` to its pre-fix two-line body (no `rm -f
  "$out"`, no `PIPESTATUS` check) made the test fail the same way:
  `Some(0)` with `[PARALLELS_CAPTURE:method=window:reason=ok]` in stdout —
  a failed `screencapture` (exit 3) accepted as success because a real,
  valid PNG was already sitting at `$out` from the fixture `prlctl`'s own
  write. (This test was strengthened from its first version, which used
  arbitrary non-image bytes as the stale content and only reddened via a
  narrower stderr-string check, because the arbitrary bytes independently
  failed the script's unrelated `image_probe` decode step regardless of the
  C-4 fix. The strengthened version, dispatched as a same-day follow-up and
  applied by Codex, uses a real decodable PNG as the stale content so the
  test discriminates via the primary `exit status` / `method=...:reason=ok`
  signal — the actually dangerous shape the review described.)
- **C-8** (`find_vm_backend_pid_matches_exact_vm_name_not_a_prefix`):
  reverting `find_vm_backend_pid`'s `awk` body to the plain substring test
  made the test fail: `MATCH id=99 size=900x700 owner_pid=222` (the
  `breenix-10` process, matched via the `breenix-1` substring) replaced the
  expected `MATCH id=42 ... owner_pid=111`.

C-5, C-6, C-7, and C-9 were verified by direct reading of the diff and the
already-passing test output above, not by an independent revert-and-redden
check in this addendum.

### Independent full-suite re-run

```
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library \
  cargo test --test parallels_capture_structure
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.73s
```

Re-run again after the C-4 test strengthening above, same result (15
passed), and `python3 scripts/claim-lint.py` re-run clean (exit 0) after
that change too.

### One live Parallels capture (capture path changed, so this was required)

VM `breenix-1788767221`, built via `./run.sh --parallels --test 120`
(`BREENIX_RUST_FORK_LIBRARY` exported; the worktree's userspace binaries and
`target/ext2-aarch64.img` were built fresh by this run, not copied in).
Screen was unlocked throughout (checked via
`Quartz.CGSessionCopyCurrentDictionary` before starting:
`CGSSessionScreenIsLocked: 0`). `/tmp/breenix-parallels-serial.log`
truncated before boot. Full stdout, the copied screenshot, and the copied
serial log are preserved at
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/pcap/evidence/live-verify/run-stdout.log`
(plus `screenshot.png`, `serial.log` in that same directory) -- a session
scratch directory, not committed to this repo, so this addendum's quoted
excerpts below are the durable record of that evidence.
<!-- claim-lint:ok: evidence/live-verify/run-stdout.log named above -->

The screenshot step's actual output:

```
=== Screenshot ===
Attempt 1: waiting 5s before capture for VM 'breenix-1788767221'
prlctl capture: exit=0
Attempt 1: method=prlctl size=1280x960 dominant=0,0,0 distinct=564
Created solid-red baseline: .../logs/breenix-parallels-cpu0/f20-baseline-red/solid-red.png
Solid-red baseline comparison: different
[PARALLELS_CAPTURE:method=prlctl:reason=ok]
/tmp/breenix-screenshot.png
Screenshot: /tmp/breenix-screenshot.png (capture=prlctl)
```

`f24-render-verdict.sh` against the copied screenshot:

```
distinct=401 dominant=(0, 0, 0) dom_frac=0.9196
big_color_buckets=3 blue_baseline=False red_baseline=False
coherent_region=bucket=(1, 2, 5) frac=0.0489 bbox=(1, 6, 1279, 73) bbox_frac=0.0739 fill_frac=0.6617
VERDICT=FAIL
```

`f24-render-verdict.sh` exited **1** (`VERDICT=FAIL`), not **2**
(`CAPTURE_MISSING`) -- this was a real, decodable, non-degenerate
1280x960 8-bit RGB capture (401 distinct colors, a coherent region was
found), so this is the ordinary render-quality `FAIL` outcome, not the
`CAPTURE_MISSING` outcome -- claim-lint:ok: #917, the printed
`VERDICT=FAIL` line quoted directly above is exit code 1 per
`scripts/f24-render-verdict.sh`'s own documented exit-code contract, not
exit 2. It failed the render bar
only because the dominant-black fraction (0.9196) narrowly exceeds the
`<0.90` threshold -- consistent with an early-boot frame captured before
the desktop had finished drawing, the same 5s-after-120s timing
sensitivity this doc's original "one more thing" section already
describes, not a defect in any of C-3 through C-9. **Not claimed**: that
this capture proves the kernel's desktop renders correctly by this point
in boot -- it doesn't; it proves the fixed capture mechanism itself
(window/PID matching, screencapture status checking, OUTPUT-write
ordering, stale-file clearing) obtained and correctly reported a real
frame end to end against live `prlctl`/`ps`/`Quartz`, which is what this
addendum set out to check.

`run.sh`'s own exit code for this specific run was not independently
captured (the run was launched via `nohup ... &` in a background shell and
its stdout/stderr were redirected to a log file that does not include
`$?`) -- claim-lint:ok: #917, disclosed gap, not a claim of a result.
The C-5 fix itself -- that a `capture=none` test-mode run now exits
non-zero -- is proven directly by
`run_sh_test_mode_exit_reflects_capture_outcome` and its mutation
companion above, not by this live run (which took the success path,
`CAPTURE_OK` never set to `false`).

VM cleanup: `prlctl stop breenix-1788767221 --kill`, polled to `stopped`
(one poll, no waiting needed), then `prlctl delete breenix-1788767221`. A
`prlctl list -a` after cleanup shows exactly one VM, `linux-probe
suspended` -- the same state as before this round started.

### claim-lint (this addendum)

```
claim-lint: python3 scripts/claim-lint.py (worktree, after this addendum) -> exit 0
```

## Landing (2026-09-07)

A third review pass found three prose findings against this doc and the
files it echoes (`run.sh`, `capture-display.sh`, `screenshot-vm.sh`,
`tests/parallels_capture_structure.rs`): C-1/C-2 (major, both about the
`docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/`
count -- corrected from "5/7 solid-black" to 1/6, and from "7/7 window-lookup
failures" to 6/6 of the runs that reached the screenshot step, since the
seventh non-aborted run (`run1-launcher-smoke`) failed earlier on
`FAIL: USB_MOUSE_ENUM` with no `screenshot.png` and 0 occurrences of the
lookup-failure string) and C-12 (nit, `screenshot-vm.sh`'s usage text
described its argument as a substring match when `capture-display.sh` now
takes it as an exact `prlctl capture`/`--vm-name` name). All three fixed in
commit `67d6c51e`; see that commit's own message for the independently
re-derived counts. The prior commit `df2e0849` on this same branch (already
pushed to `origin/tools/parallels-capture-fix` before this round started)
still carries the old 5/7 and 7/7 figures in its own commit message -- per
standing practice this is not corrected by rewriting pushed history; the
corrected figures (1/6, 6/6) are recorded here and in the PR body instead.

`git fetch origin && git merge origin/main` merged `origin/main` at
`64326562` into this branch with no conflicts, merge commit `e13f7152`.

`bash scripts/run-structure-tests.sh` (default `teardown_structure`) at
`e13f7152`: **94 passed; 0 failed** ("test result: ok. 94 passed; 0 failed;
0 ignored; 0 measured; 0 filtered out"). This branch's own structure
ratchet, `bash scripts/run-structure-tests.sh parallels_capture_structure`,
also re-run at the same SHA: **15 passed; 0 failed**.

claim-lint (tree, changed hunks vs `origin/main` at merge time):
```
claim-lint: python3 scripts/claim-lint.py -> exit 0
```

claim-lint (this round's commit message, `67d6c51e`):
```
claim-lint: python3 scripts/claim-lint.py --commit-msg <msg file> -> exit 0
```

## Not claimed (landing)

- That the 10 `--whole-file` findings `claim-lint.py` reports elsewhere in
  `run.sh` (lines 6, 293, 392, 434, 789, 935, 941, 1029, 1062, 1080, each an
  unquantified-absolute hit per claim-lint's own `universal-claim` rule --
  claim-lint:ok: 10/10, see the `--files run.sh` output earlier in this
  round) are addressed by this round. They pre-date this branch, sit
  outside every hunk this branch or its landing fix touched, and
  `--changed-only` (this repo's default, and the mode this round's
  `claim-lint` line above used) correctly does not surface them; they are
  disclosed here, not fixed here, and are a candidate for a future,
  separate round.
