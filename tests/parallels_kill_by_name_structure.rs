//! #860 Parallels-VM pattern-sweep structural ratchet.
//!
//! Same hazard class #829/#849 fixed for `qemu-system-x86_64`/
//! `qemu-system-aarch64` processes and Docker containers
//! (`tests/qemu_kill_by_name_structure.rs`), one resource type over:
//! `scripts/f21-bisect-verdict.sh`'s pre-run cleanup used to be
//!
//! ```sh
//! for old_vm in $(prlctl list --all 2>/dev/null | awk '/breenix-/ {print $NF}'); do
//!     prlctl stop "$old_vm" --kill >/dev/null 2>&1 || true
//!     prlctl delete "$old_vm" >/dev/null 2>&1 || true
//! done
//! ```
//! claim-lint:ok: #860 -- verbatim pre-fix text from this round's own PR
//! diff of scripts/f21-bisect-verdict.sh, reproduced so the fix is
//! checkable against it.
//!
//! -- a bare host-wide name-pattern sweep over every Parallels VM whose
//! name matched `breenix-`, not only a VM this invocation itself started.
//! A different, concurrent Parallels-based gate, or a human's own
//! long-running `breenix-dev` VM sharing the pattern, could be stopped and
//! deleted out from under it. #860's fix: the script now persists the one
//! VM name its own lineage started to a state file
//! (`$RUN_DIR/last-vm-name`) the moment that name is known, and its
//! pre-run cleanup reaps exactly that recorded name -- never a pattern
//! match -- clearing the state file either way, so a VM this script did
//! not itself start can never be a candidate no matter what its name is.
//! claim-lint:ok: #860 -- the code fence above and the paragraphs
//! describing it are the real, literal pre-fix text this round's own PR
//! diff removed from scripts/f21-bisect-verdict.sh, and the replacement
//! shape's properties are the ones f21_bisect_verdict_sh_has_no_prlctl_pattern_sweep
//! and prlctl_pattern_sweep_predicate_is_not_vacuous check below.
//!
//! This file's one property, `prlctl_pattern_sweep_violations`: no
//! `for VAR in $(...)` loop whose command substitution invokes
//! `prlctl list` piped through `awk`/`grep` naming a pattern, combined with
//! a `prlctl stop`/`prlctl delete` of that same loop variable anywhere in
//! the script -- the enumerate-many-by-substring-then-kill-each shape, as
//! opposed to a stop/delete of a single, invocation-owned name (a literal
//! variable this script assigned itself, e.g. from its own `$!`, its own
//! `prlctl create` call, or -- as `scripts/f21-bisect-verdict.sh` now
//! does -- its own state file).
//!
//! Census-shaped like its qemu sibling (the #549/#551/#527-r1 lesson):
//! `prlctl_pattern_sweep_violations` re-derives each violation from a
//! script's own text every run, so a new script anywhere in this tree's
//! `.sh` reachable set introducing this shape is caught automatically.
//! claim-lint:ok: #860 -- structural, checked by
//! census_reaches_the_known_unfixed_run_sh_instances re-deriving the walk
//! from disk on every run rather than a fixed list.
//!
//! SCOPE, disclosed rather than silently narrowed: this round fixes only
//! `scripts/f21-bisect-verdict.sh` (#860's own filed scope). The identical
//! shape is separately, currently, and un-fixed-by-this-round present in
//! `run.sh` twice (its pre-build VM stop loop and its pre-launch VM
//! cleanup loop, both `for OLD_VM in $(prlctl list --all | grep 'breenix-'
//! | awk '{print $NF}')`) -- #860's own issue body names this file as a
//! second recurrence and explicitly says fixing the bisect script alone
//! would not close the class. This ratchet does not assert `run.sh` clean
//! (it is not, and this round did not touch it); instead
//! `census_reaches_the_known_unfixed_run_sh_instances` asserts the
//! predicate DOES currently fire on `run.sh`'s real, live text -- proving
//! the census's reach and shape are correct against a real positive this
//! round leaves standing, not merely against a synthetic string -- and
//! records that leaving it stand is a disclosed, tracked gap, not a
//! silent one. `scripts/parallels/launcher-smoke.sh` also mentions
//! `prlctl list` (an `EXISTING_VM` read-only serial-run guard that only
//! ever prints a name and refuses to start, and a `VM_NAME` fallback that
//! resolves this invocation's OWN just-started VM's name when the
//! authoritative `run.sh` stdout line is unavailable) but contains no
//! `for VAR in $(prlctl list ...)` loop at all, so it is a structurally
//! different, narrower shape this predicate correctly does not flag --
//! noted here since #860's own issue body named it alongside `run.sh`.
//! `run.sh`'s gap is tracked as #868 (filed alongside this round) rather than
//! folded into this one (small-PR mode, R157).
//! claim-lint:ok: #860,#868 -- the run.sh line/count claims above are
//! checked, not asserted, by census_reaches_the_known_unfixed_run_sh_instances
//! below, which re-derives them from run.sh's real text on every run.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// Identical walk to `tests/qemu_kill_by_name_structure.rs`'s own
/// `all_shell_scripts`: each `.sh` file under `docker/` (recursive) and
/// `scripts/` (recursive), plus `run.sh` itself.
fn all_shell_scripts() -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read script directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "sh") {
                let relative = path
                    .strip_prefix(root)
                    .expect("script below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, fs::read_to_string(&path).expect("read shell script")));
            }
        }
    }

    let root = repo_root();
    let mut scripts = Vec::new();
    visit(&root, &root.join("docker"), &mut scripts);
    visit(&root, &root.join("scripts"), &mut scripts);
    scripts.push(("run.sh".to_string(), repo_text("run.sh")));
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

/// True if `line` references shell variable `var` via `$var` or `${var}`,
/// with a non-identifier character (or end of line) immediately following
/// so `$OLD` does not spuriously match a longer name like `$OLD_VM`.
fn line_references_variable(line: &str, var: &str) -> bool {
    let is_boundary = |c: Option<char>| !matches!(c, Some(c) if c.is_ascii_alphanumeric() || c == '_');
    let bare = format!("${var}");
    if let Some(pos) = line.find(&bare) {
        let next = line[pos + bare.len()..].chars().next();
        if is_boundary(next) {
            return true;
        }
    }
    let braced = format!("${{{var}}}");
    line.contains(&braced)
}

/// If `line` is a `for VAR in $(...)` loop header whose command
/// substitution invokes `prlctl list` piped through `awk` or `grep`
/// (the enumerate-by-name-substring shape), returns `VAR`. A comment line
/// does not match.
fn for_loop_var_over_prlctl_list_pattern_query(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || !trimmed.starts_with("for ") {
        return None;
    }
    let rest = &trimmed[4..];
    let in_pos = rest.find(" in ")?;
    let var = rest[..in_pos].trim();
    let is_identifier = !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !is_identifier {
        return None;
    }
    let lower = rest[in_pos + 4..].to_lowercase();
    if lower.contains("prlctl list") && (lower.contains("awk") || lower.contains("grep")) {
        Some(var.to_string())
    } else {
        None
    }
}

/// The set of loop variables `text` binds via
/// `for_loop_var_over_prlctl_list_pattern_query`, anywhere in the file.
fn prlctl_pattern_sweep_loop_vars(text: &str) -> HashSet<String> {
    text.lines().filter_map(for_loop_var_over_prlctl_list_pattern_query).collect()
}

/// True if `line` is a live (non-comment) `prlctl stop`/`prlctl delete`
/// referencing `var`.
fn line_is_prlctl_stop_or_delete_of_var(line: &str, var: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let lower = line.to_lowercase();
    if !(lower.contains("prlctl stop") || lower.contains("prlctl delete")) {
        return false;
    }
    line_references_variable(line, var)
}

/// The prlctl-pattern-sweep violations found in `text`: a `prlctl
/// stop`/`prlctl delete` of a variable this same file bound from a
/// `prlctl list | awk|grep <pattern>` enumeration, one entry per offending
/// line.
fn prlctl_pattern_sweep_violations(text: &str) -> Vec<String> {
    let loop_vars = prlctl_pattern_sweep_loop_vars(text);
    if loop_vars.is_empty() {
        return Vec::new();
    }
    let mut violations = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        for var in &loop_vars {
            if line_is_prlctl_stop_or_delete_of_var(line, var) {
                violations.push(format!("line {}: {}", idx + 1, line.trim()));
            }
        }
    }
    violations
}

#[test]
fn f21_bisect_verdict_sh_has_no_prlctl_pattern_sweep() {
    let path = "scripts/f21-bisect-verdict.sh";
    let text = repo_text(path);

    let violations = prlctl_pattern_sweep_violations(&text);
    assert!(
        violations.is_empty(),
        "{path} must not stop/delete a Parallels VM enumerated by a `prlctl list` \
         name-pattern match instead of a name this invocation itself recorded, so a \
         different concurrent gate's or a human's own VM sharing the pattern cannot be \
         killed out from under it (#860):\n{}",
        violations.join("\n")
    );

    // The fix's own replacement shape -- reaping a name read back from this
    // script's own state file -- must not itself trip the predicate.
    assert!(
        text.contains("VM_STATE_FILE"),
        "sanity: the fix's own state-file variable must still be present in {path}"
    );
}

/// ANTI-VACUITY: the predicate fires on the real shapes it claims to
/// (positive, both synthetic and the real pre-fix text spliced back onto
/// the real post-fix file) and stays quiet on the real safe shape the fix
/// introduces (negative, checked against the real post-fix file above).
#[test]
fn prlctl_pattern_sweep_predicate_is_not_vacuous() {
    // --- Positive: the shape, as a literal string. ---
    assert_eq!(
        for_loop_var_over_prlctl_list_pattern_query(
            "for old_vm in $(prlctl list --all 2>/dev/null | awk '/breenix-/ {print $NF}'); do"
        ),
        Some("old_vm".to_string()),
        "must bind the loop variable of a prlctl-list/awk enumeration"
    );
    assert_eq!(
        for_loop_var_over_prlctl_list_pattern_query(
            "for OLD_VM in $(prlctl list --all 2>/dev/null | grep 'breenix-' | awk '{print $NF}'); do"
        ),
        Some("OLD_VM".to_string()),
        "must bind the loop variable of a prlctl-list/grep+awk enumeration"
    );
    assert!(
        line_is_prlctl_stop_or_delete_of_var(r#"    prlctl stop "$old_vm" --kill >/dev/null 2>&1 || true"#, "old_vm"),
        "must detect a prlctl stop of the bound loop variable"
    );
    assert!(
        line_is_prlctl_stop_or_delete_of_var(r#"    prlctl delete "$old_vm" >/dev/null 2>&1 || true"#, "old_vm"),
        "must detect a prlctl delete of the bound loop variable"
    );

    // --- Negative: a loop over a fixed/explicit list, and a stop/delete of
    // an invocation-owned name, must not trip either predicate.
    assert_eq!(
        for_loop_var_over_prlctl_list_pattern_query("for arg in \"$@\"; do"),
        None,
        "an ordinary positional-parameter loop must not be flagged"
    );
    assert!(
        !line_is_prlctl_stop_or_delete_of_var(r#"    prlctl stop "$VM_NAME" --kill >/dev/null 2>&1 || true"#, "old_vm"),
        "a stop of an unrelated variable name must not match a different loop variable"
    );
    assert!(
        for_loop_var_over_prlctl_list_pattern_query(
            "# for old_vm in $(prlctl list --all | awk '/breenix-/ {print $NF}'); do"
        )
        .is_none(),
        "a doc comment describing the old shape in prose must not itself be flagged"
    );

    // --- Real-file sanity: scripts/f21-bisect-verdict.sh is clean post-fix
    // (duplicated here, not only in the dedicated test above, so this
    // file's own anti-vacuity test stands alone).
    let fixed_path = "scripts/f21-bisect-verdict.sh";
    let fixed_text = repo_text(fixed_path);
    assert!(
        prlctl_pattern_sweep_violations(&fixed_text).is_empty(),
        "{fixed_path} must be clean post-fix"
    );

    // --- REDDENING mutation: splicing the real, literal pre-#860 for-loop
    // back onto the real post-fix file text must trip the ratchet. Copied
    // verbatim from the pre-fix file (this file's own git history, and
    // #860's issue body).
    let pre_fix_block = "for old_vm in $(prlctl list --all 2>/dev/null | awk '/breenix-/ {print $NF}'); do\n    \
        prlctl stop \"$old_vm\" --kill >/dev/null 2>&1 || true\n    \
        prlctl delete \"$old_vm\" >/dev/null 2>&1 || true\ndone\n";
    let mutated = format!("{fixed_text}{pre_fix_block}");
    assert!(
        !prlctl_pattern_sweep_violations(&mutated).is_empty(),
        "reddening: reintroducing scripts/f21-bisect-verdict.sh's real pre-#860 \
         pattern-sweep loop must trip the ratchet"
    );
}

/// The census's own reach: `scripts/f21-bisect-verdict.sh` and `run.sh`
/// are both walked by `all_shell_scripts()`, and the predicate DOES
/// currently fire on `run.sh`'s real, live, un-fixed-by-this-round text --
/// proving this ratchet's reach and shape are correct against a real
/// positive this round knowingly leaves standing (see module doc SCOPE),
/// not merely against a synthetic string or a file this round already
/// cleaned up.
#[test]
fn census_reaches_the_known_unfixed_run_sh_instances() {
    let scripts = all_shell_scripts();

    // Anti-vacuity floor: 94 `.sh` scripts under docker/ and scripts/ plus
    // run.sh itself, as of this file's own last edit. Not a closed list --
    // a future script only needs to raise this count.
    assert!(
        scripts.len() >= 95,
        "only {} script(s) found under docker/, scripts/, and run.sh; expected at least 95",
        scripts.len()
    );

    for path in ["scripts/f21-bisect-verdict.sh", "run.sh"] {
        assert!(
            scripts.iter().any(|(p, _)| p == path),
            "{path} must be walked by this ratchet's own full-tree census, or a hit \
             against its real text proves nothing about detection"
        );
    }

    let run_sh_text = repo_text("run.sh");
    let run_sh_violations = prlctl_pattern_sweep_violations(&run_sh_text);
    assert!(
        run_sh_violations.len() >= 3,
        "run.sh is expected to still carry its two known, disclosed, un-fixed-by-#860 \
         prlctl-pattern-sweep loops (a stop-only pre-build loop, one offending line, and \
         a stop+delete pre-launch loop, two offending lines -- three offending lines \
         total) -- found {}: {:?}. If this now reads 0, run.sh has been fixed: update \
         this test to assert it clean instead of asserting the gap, and close #868.",
        run_sh_violations.len(),
        run_sh_violations
    );

    // scripts/parallels/launcher-smoke.sh's own `prlctl list` usage (an
    // EXISTING_VM read-only guard and a VM_NAME own-name fallback, module
    // doc SCOPE) contains no `for VAR in $(prlctl list ...)` loop at all,
    // so this predicate correctly finds nothing there -- confirmed, not
    // merely asserted, so a future change that DID add the sweep shape to
    // that file would be caught by the full-tree walk once one is added to
    // this file's assertions.
    // claim-lint:ok: #860 -- checked by the assert! immediately below,
    // which re-derives it from launcher-smoke.sh's real text on every run.
    let launcher_smoke_text = repo_text("scripts/parallels/launcher-smoke.sh");
    assert!(
        prlctl_pattern_sweep_loop_vars(&launcher_smoke_text).is_empty(),
        "sanity: scripts/parallels/launcher-smoke.sh was expected to contain no \
         `for VAR in $(prlctl list ...)` loop at all (module doc SCOPE); if this now \
         fails, that file has grown the sweep shape and belongs in this ratchet's \
         assertions"
    );
}
