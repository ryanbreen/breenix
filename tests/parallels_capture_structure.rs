//! #917 Parallels screenshot capture path: no fallback image write, and the
//! render verdict refuses to score a capture that did not happen.
//!
//! Live diagnosis (docs/planning/green-program/gui/PARALLELS-CAPTURE-2026-09-07.md)
//! found two structural bugs in the capture path `run.sh --parallels --test N`
//! actually exercised:
//!
//! 1. `scripts/parallels/screenshot-vm.sh`'s (and the pre-fix
//!    `scripts/parallels/capture-display.sh`'s) window lookup matched the VM
//!    name as a substring of `kCGWindowName`. Live inspection of
//!    `Quartz.CGWindowListCopyWindowInfo` during a running boot, and with no
//!    Breenix VM running at all
//!    (docs/planning/green-program/gui/evidence/windowlist-no-vm.txt: 12/12
//!    Parallels-owned windows, 0/12 with a non-empty `kCGWindowName`), showed
//!    Parallels sets no per-VM window title when the VM is started via
//!    `prlctl start` with no GUI open (this harness's normal mode) -- the
//!    match cannot succeed by construction, matching the 6/6 identical
//!    failures (of the runs that reached this step) in
//!    docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/evidence/.
//! 2. `run.sh`'s test-mode screenshot step then fell back to a bare
//!    `prlctl capture "$PARALLELS_VM" --file "$SCREENSHOT" 2>/dev/null`
//!    with no retry and no content check, and reported
//!    `Screenshot: $SCREENSHOT` unconditionally on its exit 0 -- even when
//!    `prlctl capture` itself returned a genuine (not synthetic) solid-black
//!    frame because the guest compositor had not rendered anything yet.
//!
//! The fix (scripts/parallels/capture-display.sh, run.sh,
//! scripts/f24-render-verdict.sh) writes a screenshot file from exactly one
//! code path -- after a verified non-degenerate frame was captured -- and
//! separates "no capture happened" (`CAPTURE_MISSING`, exit 2) from
//! "captured the desktop and it looks wrong" (`FAIL`, exit 1) at the
//! render-verdict layer too, so a caller can distinguish the capture path's
//! own failure from the kernel's.
//!
//! This file pins three properties, each checked against the scripts' real
//! text (or, for the render verdict, against a live run of the actual
//! script) rather than against a fixed list, per the R157/#549 lesson that a
//! literal-list ratchet stops catching the class the moment the surrounding
//! code moves:
//!
//! - `capture-display.sh` writes its OUTPUT path in exactly one place, and
//!   that place is reached only after a captured frame has been verified
//!   non-black (`capture_display_writes_output_exactly_once_and_only_on_a_verified_frame`),
//!   proven non-vacuous by reinstating the historical shape -- an
//!   unconditional second write near the terminal failure path -- and
//!   checking the predicate reddens
//!   (`capture_display_output_write_check_is_not_vacuous`).
//! - `run.sh` no longer contains the silent `|| prlctl capture ... 2>/dev/null`
//!   fallback and does call the hardened `capture-display.sh`
//!   (`run_sh_has_no_silent_capture_fallback_and_calls_capture_display`),
//!   with a matching non-vacuity check that reinstates the old fallback line
//!   (`run_sh_silent_fallback_check_is_not_vacuous`).
//! - `scripts/f24-render-verdict.sh`, run live against a missing path and
//!   against a real solid-black PNG fixture (synthesized with the same
//!   python3+PIL dependency the capture pipeline itself already requires),
//!   exits 2 with `VERDICT=CAPTURE_MISSING` in both cases rather than
//!   scoring either as an ordinary FAIL
//!   (`f24_render_verdict_rejects_missing_capture`,
//!   `f24_render_verdict_rejects_solid_black_capture`) -- and still passes
//!   a real rendered-desktop fixture
//!   (`f24_render_verdict_still_passes_a_real_capture`).

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// True if `line` performs a filesystem write whose destination is the
/// script's own `$OUTPUT` variable: either `cp ... "$OUTPUT"` (OUTPUT as the
/// copy destination -- the last token) or a shell redirection into it
/// (`> "$OUTPUT"` / `>> "$OUTPUT"`). A comment line does not match. Note the
/// script's python heredocs receive `$OUTPUT`'s value as a subprocess argv
/// value instead of referencing it literally (`sys.argv[1]` etc. -- 0
/// occurrences of the literal token `$OUTPUT` inside the heredocs, checked
/// directly against the script's text below), so a plain text search for the
/// literal `"$OUTPUT"` token is precise here, not merely approximate.
fn writes_to_output(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.starts_with('#') || !trimmed.contains("\"$OUTPUT\"") {
        return false;
    }
    let is_cp_dest = trimmed.starts_with("cp ") && trimmed.ends_with("\"$OUTPUT\"");
    let is_redirect = trimmed.contains("> \"$OUTPUT\"") || trimmed.contains(">\"$OUTPUT\"");
    is_cp_dest || is_redirect
}

/// The set of lines in `script` that write to `$OUTPUT`, as (1-indexed
/// line number, trimmed line text) pairs.
fn output_write_lines(script: &str) -> Vec<(usize, String)> {
    script
        .lines()
        .enumerate()
        .filter(|(_, line)| writes_to_output(line))
        .map(|(idx, line)| (idx + 1, line.trim().to_string()))
        .collect()
}

const CAPTURE_DISPLAY: &str = "scripts/parallels/capture-display.sh";

#[test]
fn capture_display_writes_output_exactly_once_and_only_on_a_verified_frame() {
    let text = repo_text(CAPTURE_DISPLAY);

    let writes = output_write_lines(&text);
    assert_eq!(
        writes.len(),
        1,
        "{CAPTURE_DISPLAY} must write its OUTPUT path in exactly one place -- \
         the one verified-frame success path -- never a second, unconditional \
         write (that shape is what let a black or missing capture look like a \
         success, #917). Found: {writes:?}"
    );
    assert_eq!(
        writes[0].1, "cp \"$candidate\" \"$OUTPUT\"",
        "the one OUTPUT write in {CAPTURE_DISPLAY} must be the known verified-frame \
         copy, not some other shape"
    );

    // The write must sit strictly between the black-frame retry gate and the
    // terminal failure path, so exhausting the retry schedule cannot fall
    // through to writing OUTPUT by construction, regardless of what any
    // single line's text looks like in isolation.
    let black_check_line = text
        .lines()
        .position(|line| line.contains("black_delay") && line.contains("json_bool"))
        .expect("black_delay gate must be present in capture-display.sh");
    let failure_tail_line = text
        .lines()
        .position(|line| line.contains("ERROR: failed to capture"))
        .expect("terminal failure log line must be present in capture-display.sh");
    let write_line_idx = writes[0].0 - 1;
    assert!(
        write_line_idx > black_check_line,
        "the OUTPUT write (line {}) must come after the black-frame gate (line {})",
        writes[0].0,
        black_check_line + 1
    );
    assert!(
        write_line_idx < failure_tail_line,
        "the OUTPUT write (line {}) must come before the terminal failure path (line {})",
        writes[0].0,
        failure_tail_line + 1
    );
}

/// Non-vacuity proof for the check above: reinstating the historical
/// failure-mode shape (an unconditional second write to OUTPUT sitting near
/// the terminal failure path -- what a "just write something so run.sh has
/// a file to point at" fallback would look like) must flip
/// `output_write_lines` from one match to more than one. This is the
/// "mutation: reinstate the fallback write -> red" proof.
#[test]
fn capture_display_output_write_check_is_not_vacuous() {
    let text = repo_text(CAPTURE_DISPLAY);
    assert_eq!(output_write_lines(&text).len(), 1, "sanity: real file has exactly one OUTPUT write");

    let anchor = "log \"ERROR: failed to capture a non-black Parallels display for VM '$VM_NAME' (reason=$last_reason)\"";
    assert!(
        text.contains(anchor),
        "mutation anchor text must actually appear in {CAPTURE_DISPLAY} (the file's own text moved out from under this test)"
    );
    // The injected write sits on its own line -- an inline trailing comment
    // after it would break the ends_with("\"$OUTPUT\"") check this same
    // function relies on to find it, silently defeating the whole point of
    // this mutation probe.
    let mutated = text.replace(
        anchor,
        &format!("# reinstated fallback write (mutation probe)\ncp \"$candidate\" \"$OUTPUT\"\n{anchor}"),
    );
    assert_ne!(mutated, text, "mutation must actually change the text");

    let mutated_writes = output_write_lines(&mutated);
    assert!(
        mutated_writes.len() > 1,
        "reinstating an unconditional fallback OUTPUT write near the terminal failure \
         path must be caught (mutation did not redden the check): {mutated_writes:?}"
    );
}

const RUN_SH: &str = "run.sh";

/// The historical #917 shape in run.sh's test-mode screenshot step: a bare
/// `prlctl capture` OR-fallback after the screenshot-vm.sh window lookup
/// described in this file's module doc above, piping stderr to /dev/null and
/// then reporting success on exit 0 regardless of what was actually
/// captured.
fn has_silent_prlctl_capture_fallback(text: &str) -> bool {
    text.contains("|| prlctl capture \"$PARALLELS_VM\" --file \"$SCREENSHOT\" 2>/dev/null")
}

#[test]
fn run_sh_has_no_silent_capture_fallback_and_calls_capture_display() {
    let text = repo_text(RUN_SH);
    assert!(
        !has_silent_prlctl_capture_fallback(&text),
        "{RUN_SH} must not silently accept whatever `prlctl capture` returns \
         without validating the frame it captured (#917)"
    );
    assert!(
        text.contains(CAPTURE_DISPLAY),
        "{RUN_SH}'s test-mode screenshot step must call the hardened {CAPTURE_DISPLAY} \
         (#917: screenshot-vm.sh's window lookup is a guaranteed miss in this harness's mode)"
    );
    assert!(
        text.contains("capture=none"),
        "{RUN_SH} must be able to report capture=none rather than an unconditional \
         \"Screenshot: ...\" success line when no real frame was captured (#917)"
    );
}

/// Non-vacuity check (claim-lint:ok: #917): reinstating the old
/// silent-fallback line anywhere in run.sh's text must flip
/// `has_silent_prlctl_capture_fallback` from false to true.
#[test]
fn run_sh_silent_fallback_check_is_not_vacuous() {
    let text = repo_text(RUN_SH);
    assert!(!has_silent_prlctl_capture_fallback(&text), "sanity: real file has no silent fallback");

    let mutated = format!(
        "{text}\n# mutation probe\nfoo() {{ true || prlctl capture \"$PARALLELS_VM\" --file \"$SCREENSHOT\" 2>/dev/null; }}\n"
    );
    assert!(
        has_silent_prlctl_capture_fallback(&mutated),
        "reinstating the old silent-fallback shape must be caught (mutation did not redden the check)"
    );
}

const F24_VERDICT: &str = "scripts/f24-render-verdict.sh";

#[test]
fn f24_render_verdict_rejects_missing_capture() {
    let root = repo_root();
    let missing = root.join("target").join(format!("breenix-f24-missing-capture-probe-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&missing); // must not exist

    let out = Command::new("bash")
        .arg(root.join(F24_VERDICT))
        .arg(&missing)
        .output()
        .expect("spawn f24-render-verdict.sh");

    assert_eq!(
        out.status.code(),
        Some(2),
        "a missing capture PNG must exit 2 (CAPTURE_MISSING), not FAIL(1) or a false PASS(0). \
         stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("VERDICT=CAPTURE_MISSING"), "stdout was: {stdout}");
}

/// Synthesizes a real solid-black 1280x960 PNG (the same shape as the #917
/// evidence capture -- see docs/planning/green-program/sweeps/
/// input-gui-aarch64-2026-09-06/evidence/run3-test120-1/screenshot.png,
/// same dominant color and dimensions though not byte-for-byte, since this
/// test does not depend on this repository's evidence files continuing to
/// exist) and asserts the render verdict refuses to score it as an ordinary
/// FAIL.
#[test]
fn f24_render_verdict_rejects_solid_black_capture() {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!("breenix-f24-black-capture-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp fixture dir");
    let png = dir.join("solid-black.png");

    let make = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "from PIL import Image; Image.new('RGB', (1280, 960), (0, 0, 0)).save({:?})",
            png.to_string_lossy()
        ))
        .output()
        .expect("spawn python3 to synthesize the black PNG fixture");
    assert!(
        make.status.success(),
        "failed to synthesize the black PNG fixture (python3+PIL is already a hard \
         dependency of the capture pipeline this test exercises): {}",
        String::from_utf8_lossy(&make.stderr)
    );

    let out = Command::new("bash")
        .arg(root.join(F24_VERDICT))
        .arg(&png)
        .output()
        .expect("spawn f24-render-verdict.sh");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        out.status.code(),
        Some(2),
        "a solid-black capture must exit 2 (CAPTURE_MISSING), not an ordinary FAIL(1) -- \
         FAIL means \"captured the desktop and it looks wrong\", this is \"never captured \
         the desktop at all\". stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("VERDICT=CAPTURE_MISSING"), "stdout was: {stdout}");
    assert!(stdout.contains("solid-black-capture"), "stdout was: {stdout}");
}

/// Regression guard alongside the two rejections above: a real rendered
/// (non-degenerate) capture must still PASS through unchanged. Synthesizes
/// a fixture with enough distinct, spatially-coherent color content to
/// satisfy the existing render-verdict thresholds (distinct>20, 3+ big
/// color buckets, a coherent non-background region) so this proves the
/// CAPTURE_MISSING preflight this round added does not also swallow a
/// legitimate PASS.
#[test]
fn f24_render_verdict_still_passes_a_real_capture() {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!("breenix-f24-real-capture-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp fixture dir");
    let png = dir.join("rendered.png");

    let script = format!(
        "from PIL import Image, ImageDraw\n\
         img = Image.new('RGB', (1280, 960), (10, 10, 25))\n\
         d = ImageDraw.Draw(img)\n\
         d.rectangle([100, 500, 700, 860], fill=(60, 60, 90))\n\
         d.rectangle([750, 500, 1050, 700], fill=(200, 90, 40))\n\
         d.rectangle([100, 100, 350, 300], fill=(40, 160, 120))\n\
         for i in range(25):\n\
         \x20   x = 900 + (i % 5) * 60\n\
         \x20   y = 100 + (i // 5) * 60\n\
         \x20   color = ((i * 37 + 20) % 200 + 30, (i * 61 + 50) % 200 + 30, (i * 89 + 10) % 200 + 30)\n\
         \x20   d.rectangle([x, y, x + 45, y + 45], fill=color)\n\
         img.save({:?})\n",
        png.to_string_lossy()
    );
    let make = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("spawn python3 to synthesize the rendered-desktop fixture");
    assert!(
        make.status.success(),
        "failed to synthesize the rendered fixture: {}",
        String::from_utf8_lossy(&make.stderr)
    );

    let out = Command::new("bash")
        .arg(root.join(F24_VERDICT))
        .arg(&png)
        .output()
        .expect("spawn f24-render-verdict.sh");
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a real rendered capture must still PASS (exit 0); the CAPTURE_MISSING preflight \
         must not swallow it. stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("VERDICT=PASS"), "stdout was: {stdout}");
}


/// Round-2 (#917 fix-pass) regressions exercise the real capture script with
/// fixture capture commands and window inventories, plus the caller's exit
/// gate with a mutation companion. No fixture starts or stops a VM.
mod round_two {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("breenix-pcap-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let fixture = Self { dir };
            fixture.bin("prlctl", "exit 255");
            fixture.bin("ps", "exit 0");
            fixture.bin("screencapture", r#"for out in "$@"; do :; done
cp "$FIXTURE_DIR/frame.png" "$out""#);
            std::fs::write(fixture.dir.join("Quartz.py"), r#"
kCGWindowListOptionAll = 0
kCGNullWindowID = 0
def CGWindowListCopyWindowInfo(*args):
    return [
        dict(kCGWindowOwnerName="Parallels Desktop", kCGWindowOwnerPID=222,
             kCGWindowNumber=99, kCGWindowLayer=0,
             kCGWindowBounds=dict(Width=900, Height=700)),
        dict(kCGWindowOwnerName="Parallels Desktop", kCGWindowOwnerPID=111,
             kCGWindowNumber=42, kCGWindowLayer=0,
             kCGWindowBounds=dict(Width=818, Height=801)),
        dict(kCGWindowOwnerName="Parallels Desktop", kCGWindowOwnerPID=333,
             kCGWindowNumber=77, kCGWindowLayer=0,
             kCGWindowBounds=dict(Width=1200, Height=900)),
    ]
"#).unwrap();
            let make = Command::new("python3").arg("-c").arg(
                "from PIL import Image, ImageDraw; import sys; im=Image.new('RGB',(818,801),(40,80,120)); ImageDraw.Draw(im).rectangle((40,40,400,400),fill=(180,140,60)); im.save(sys.argv[1])"
            ).arg(fixture.dir.join("frame.png")).output().unwrap();
            assert!(make.status.success(), "{}", String::from_utf8_lossy(&make.stderr));
            fixture
        }

        fn bin(&self, name: &str, body: &str) {
            let path = self.dir.join(name);
            std::fs::write(&path, format!("#!/bin/bash\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn run(&self) -> std::process::Output {
            Command::new("bash").arg(repo_root().join(CAPTURE_DISPLAY))
                .arg("breenix-1").arg(self.dir.join("output.png"))
                .env("FIXTURE_DIR", &self.dir)
                .env("PATH", format!("{}:{}", self.dir.display(), std::env::var("PATH").unwrap()))
                .env("PYTHONPATH", format!("{}:{}", self.dir.display(), std::env::var("PYTHONPATH").unwrap_or_default()))
                .env("BREENIX_CAPTURE_RETRY_SCHEDULE", "0")
                .env("BREENIX_CAPTURE_BASELINE_DIR", self.dir.join("baseline"))
                .output().unwrap()
        }

        fn matching_ps(&self) {
            self.bin("ps", "echo '222 /Applications/Parallels/prl_vm_app --vm-name breenix-10 --arg'\necho '111 /Applications/Parallels/prl_vm_app --vm-name breenix-1 --arg'");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).expect("remove fixture directory");
        }
    }

    #[test]
    fn find_vm_backend_pid_matches_exact_vm_name_not_a_prefix() {
        let f = Fixture::new("c8");
        f.matching_ps();
        let out = f.run();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("method=window:reason=ok"), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("MATCH id=42 size=818x801 owner_pid=111"), "{stderr}");
        assert!(!stderr.contains("MATCH id=99"), "{stderr}");
    }

    #[test]
    fn capture_window_requires_pid_match_not_any_parallels_window() {
        let f = Fixture::new("c3");
        let out = f.run();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains("method=window:reason=ok"), "{stdout}");
        assert!(stdout.contains("method=none"), "{stdout}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("NO_MATCH"), "{out:?}");
    }

    /// A real decodable stale image makes the C-4 revert fail on capture_window accepting failed screencapture, not image_probe rejecting a placeholder.
    #[test]
    fn capture_window_does_not_report_success_when_screencapture_fails() {
        let f = Fixture::new("c4");
        f.matching_ps();
        f.bin("prlctl", r#"while [ "$#" -gt 0 ]; do
    if [ "$1" = --file ]; then
        shift
        cp "$FIXTURE_DIR/frame.png" "$1"
        exit 1
    fi
    shift
done
exit 1"#);
        f.bin("screencapture", "exit 3");
        let out = f.run();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("method=window:reason=ok"), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("screencapture exited 3"), "{out:?}");
        assert!(!f.dir.join("output.png").exists());
    }

    #[test]
    fn capture_display_reports_success_when_baseline_write_fails_after_a_real_copy() {
        let f = Fixture::new("c6");
        f.bin("prlctl", r#"while [ "$#" -gt 0 ]; do
    if [ "$1" = --file ]; then
        shift
        cp "$FIXTURE_DIR/frame.png" "$1"
        exit $?
    fi
    shift
done
exit 1"#);
        std::fs::write(f.dir.join("baseline"), "blocker").unwrap();
        let out = f.run();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("method=prlctl:reason=ok"), "{stdout}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("WARNING: write_baseline_and_stats failed"), "{out:?}");
        assert!(stdout.contains(f.dir.join("output.png").to_str().unwrap()), "{stdout}");
        assert_eq!(std::fs::read(f.dir.join("output.png")).unwrap(), std::fs::read(f.dir.join("frame.png")).unwrap());
    }

    #[test]
    fn capture_display_clears_stale_output_before_a_failing_run() {
        let f = Fixture::new("c7");
        std::fs::write(f.dir.join("output.png"), b"stale prior screenshot bytes").unwrap();
        let out = f.run();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(!f.dir.join("output.png").exists());
    }

    #[test]
    fn f24_render_verdict_rejects_corrupt_png() {
        let f = Fixture::new("c9");
        let png = f.dir.join("corrupt.png");
        std::fs::write(&png, b"not a png").unwrap();
        let out = Command::new("bash").arg(repo_root().join(F24_VERDICT)).arg(png).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("VERDICT=CAPTURE_MISSING"), "{stdout}");
        assert!(stdout.contains("unreadable-image"), "{stdout}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("Traceback"), "{out:?}");
    }

    const EXIT_GATE: &str = "    if [ \"$PARALLELS_TEST\" = true ] && [ \"$CAPTURE_OK\" != true ]; then\n        exit 1\n    fi\n";

    fn run_sh_exits_nonzero_on_capture_failure(text: &str) -> bool {
        let lines: Vec<_> = text.lines().collect();
        let Some(init) = lines.iter().position(|line| line.trim() == "CAPTURE_OK=true") else { return false };
        let Some(failure) = lines.iter().position(|line| line.trim() == "CAPTURE_OK=false") else { return false };
        let Some(gate) = lines.iter().position(|line| *line == EXIT_GATE.lines().next().unwrap()) else { return false };
        let next = lines.iter().skip(gate + 1).find(|line| !line.trim().is_empty());
        init < failure && failure < gate && next == Some(&"        exit 1")
            && lines.iter().skip(gate + 1).any(|line| *line == "    exit 0")
    }

    #[test]
    fn run_sh_test_mode_exit_reflects_capture_outcome() {
        assert!(run_sh_exits_nonzero_on_capture_failure(&repo_text(RUN_SH)));
    }

    #[test]
    fn run_sh_capture_exit_check_is_not_vacuous() {
        let text = repo_text(RUN_SH);
        assert!(run_sh_exits_nonzero_on_capture_failure(&text));
        assert!(text.contains(EXIT_GATE));
        let mutated = text.replace(EXIT_GATE, "");
        assert_ne!(text, mutated);
        assert!(!run_sh_exits_nonzero_on_capture_failure(&mutated));
    }
}
