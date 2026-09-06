//! #829/#849 kill-by-name structural ratchet.
//!
//! #829's own report: `docker/qemu/run-aarch64-test.sh` and
//! `docker/qemu/run-aarch64-userspace.sh` each ended their cleanup with
//! `docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64)` --
//! a match on the *image*, not the container this invocation itself
//! started, so the first of two concurrent invocations to finish could
//! kill the second invocation's still-running container out from under
//! it. #829's own comment thread found the identical shape a second time
//! (`docker/qemu/run-aarch64-interactive.sh`'s pre-acquire `docker kill
//! $EXISTING`), and this campaign's own prior round (#834's F2) found and
//! fixed a third instance in a sibling file
//! (`docker/qemu-aarch64/run-arm64-boot.sh`, via a `--name`-scoped
//! container this file's own predicates confirm stays clean). Auditing
//! that fix round's own reachable set (each script that cooperates with
//! `docker/qemu/lib/qemu-host-lock.sh`, the same set
//! `tests/qemu_host_lock_structure.rs` already walks) found a fourth,
//! different-shaped instance: `scripts/run-arm64-boot-test.sh`'s
//! `pkill -9 -f "qemu-system-aarch64.*kernel-aarch64"` pre-launch cleanup.
//! That round's ratchet (this file, as first landed) scoped its reachable
//! set to that same lock-cooperator set, and disclosed rather than
//! silently absorbed ten x86-side instances of the identical shape sitting
//! outside it -- `qemu-system-x86_64` has no host-wide lock analogous to
//! `qemu-host-lock.sh`, so `sources_qemu_host_lock` could never reach a
//! script that launches it.
//! claim-lint:ok: #849
//!
//! #849 fixed those ten x86-side scripts (`docker/qemu/run-boot-parallel.sh`,
//! `docker/qemu/run-kthread-test.sh`, `docker/qemu/run-kthread-parallel.sh`,
//! `docker/qemu/run-dns-test.sh`, `docker/qemu/run-blocking-recv-test.sh`,
//! `docker/qemu/run-nonblock-eagain-test.sh`, `docker/qemu/run-interactive.sh`,
//! `scripts/test-workqueue.sh`, `scripts/f21-bisect-verdict.sh`,
//! `scripts/ci/ring3_check.sh`) and widened this file's own reachable set to
//! match: every `.sh` script under `docker/`, `scripts/`, or `run.sh` --
//! not only the ones that source the aarch64 lock. A script needs no
//! shared lock to owe this property; it only needs to be capable of
//! running concurrently with another script on the same host, which any
//! script in this tree can.
//! claim-lint:ok: #849
//!
//! This file's one property: no `.sh` script under `docker/`, `scripts/`,
//! or `run.sh` kills a qemu-ish process or container by name/pattern
//! rather than by an identifier (a PID from its own `$!`, or a container
//! name/id it minted for itself) this invocation captured. Three shapes,
//! matching #829's own parenthetical and the real instances found above:
//!
//! 1. `pkill`/`killall` naming a qemu-ish pattern. `pkill -P <pid>` (kill
//!    the children of a specific PID) is excluded -- already PID-scoped,
//!    not name-based; this tree's one real instance
//!    (`docker/qemu/run-vmware-gate.sh`) does not even mention "qemu",
//!    so it is excluded by the "qemu" requirement alone, but the `-P`
//!    guard keeps the predicate correct independent of that.
//! 2. `kill` fed from a `pgrep` search naming a qemu-ish pattern.
//! 3. `docker kill`/`docker stop` fed from a `docker ps` query (an
//!    ancestor/image filter), either inline
//!    (`docker kill $(docker ps -q --filter ancestor=...)`) or via a
//!    variable this same file assigns from exactly that query elsewhere
//!    (`EXISTING=$(docker ps ... --filter ancestor=...)` ...
//!    `docker kill $EXISTING`) -- checked as a same-file dataflow, not a
//!    same-line one, since the real `run-aarch64-interactive.sh` and
//!    `run-interactive.sh` instances each split the query and the kill
//!    across two lines.
//!
//! Census-shaped, not a closed file list (the #549/#551/#527-r1 lesson):
//! `kill_by_name_of_qemu_violations` re-derives each violation from a
//! script's own text each time it runs, so a new script anywhere in this
//! tree's `.sh` reachable set that introduces any of the three shapes is
//! caught automatically, not only the fourteen fixed across #829 and #849.
//!
//! Deliberately NOT covered by this file, disclosed rather than silently
//! excluded: this file's own predicates only ever look at `.sh` text, so
//! the identical `pkill -f qemu-system-x86_64` shape one layer down in
//! `scripts/breenix_runner.py`'s own `start()` (invoked by
//! `scripts/ci/ring3_check.sh`, one of #849's own ten) sits outside any
//! walk this file performs; filed as #854 rather than silently absorbed.
//! See `docs/planning/green-program/gates/KILL-BY-NAME-829-2026-09-05.md`
//! (the #829 round) and this file's own #849 round doc for both rounds'
//! full evidence.

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

/// Identical walk to `tests/qemu_host_lock_structure.rs`'s own
/// `all_shell_scripts`: each `.sh` file under `docker/` (recursive) and
/// `scripts/` (recursive), plus `run.sh` itself. This is this file's own
/// full reachable set as of #849 -- no longer filtered down to lock
/// cooperators (see module doc).
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

/// Whether a script sources the shared aarch64 host lock -- the same
/// membership test `tests/qemu_host_lock_structure.rs` uses. No longer
/// used to narrow this file's own main reachable set (#849 widened that to
/// every script -- see module doc), but kept for the aarch64-specific
/// reach-check below, which still wants to confirm the four #829-fixed
/// files are lock cooperators as a sanity check on that round's own claim.
/// claim-lint:ok: #849
fn sources_qemu_host_lock(script: &str) -> bool {
    script.contains("qemu-host-lock.sh")
}

/// Variable names this script assigns from a `docker ps ... --filter
/// ancestor=...` query -- the ancestor-image-based container lookup #829
/// itself flags. Comment lines are excluded so a doc comment describing
/// the old shape in prose cannot itself be mistaken for a live assignment.
fn variables_assigned_from_docker_ps_ancestor_filter(text: &str) -> HashSet<String> {
    let mut vars = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(eq_pos) = trimmed.find('=') else {
            continue;
        };
        let name = trimmed[..eq_pos].trim();
        let is_identifier = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_identifier {
            continue;
        }
        let rest = trimmed[eq_pos + 1..].to_lowercase();
        if rest.contains("docker ps") && rest.contains("--filter") && rest.contains("ancestor=") {
            vars.insert(name.to_string());
        }
    }
    vars
}

/// True if `line` references shell variable `var` via `$var` or `${var}`,
/// with a non-identifier character (or end of line) immediately following
/// so `$EXISTING` does not spuriously match a longer name like
/// `$EXISTINGTHING`.
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

/// Shape 1: `pkill`/`killall` naming a qemu-ish pattern, not `-P`-scoped
/// (parent-PID targeting, which names no pattern at all).
fn line_is_pkill_or_killall_by_name(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let lower = line.to_lowercase();
    let has_verb = lower.contains("pkill") || lower.contains("killall");
    has_verb && lower.contains("qemu") && !lower.contains("-p ")
}

/// Shape 2: a `kill` fed by a `pgrep` search naming a qemu-ish pattern,
/// on one line (`kill $(pgrep ... qemu ...)`, `pgrep ... qemu ... | xargs
/// kill`). The lock helper's own `pgrep -x qemu-system-aarch64` count
/// lines carry no `kill` token and so do not match.
fn line_is_pgrep_pipe_to_kill_by_name(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let lower = line.to_lowercase();
    lower.contains("pgrep") && lower.contains("qemu") && lower.contains("kill")
}

/// Shape 3: `docker kill`/`docker stop` fed from a `docker ps` ancestor
/// query, inline or via a variable this same file populates from exactly
/// that query (see `variables_assigned_from_docker_ps_ancestor_filter`).
fn line_is_docker_kill_or_stop_by_filter(line: &str, filter_vars: &HashSet<String>) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let lower = line.to_lowercase();
    if !(lower.contains("docker kill") || lower.contains("docker stop")) {
        return false;
    }
    if lower.contains("docker ps") {
        return true;
    }
    filter_vars.iter().any(|var| line_references_variable(line, var))
}

/// The kill-by-name-of-qemu lines found in `text`, one entry per offending line.
fn kill_by_name_of_qemu_violations(text: &str) -> Vec<String> {
    let filter_vars = variables_assigned_from_docker_ps_ancestor_filter(text);
    let mut violations = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line_is_pkill_or_killall_by_name(line)
            || line_is_pgrep_pipe_to_kill_by_name(line)
            || line_is_docker_kill_or_stop_by_filter(line, &filter_vars)
        {
            violations.push(format!("line {}: {}", idx + 1, line.trim()));
        }
    }
    violations
}

/// The aarch64 host-lock cooperator set #829's own round policed --
/// retained for the aarch64-specific reach-check sanity assertion below,
/// not for this file's own main reachable set (#849 widened that to every
/// script; see module doc and `all_shell_scripts`).
/// claim-lint:ok: #849
fn lock_cooperator_scripts() -> Vec<(String, String)> {
    all_shell_scripts()
        .into_iter()
        .filter(|(_, text)| sources_qemu_host_lock(text))
        .collect()
}

#[test]
fn no_shell_script_kills_qemu_by_name_or_pattern() {
    let scripts = all_shell_scripts();

    // Anti-vacuity floor: 95 `.sh` scripts exist under docker/, scripts/,
    // and run.sh as of this file's own last edit (94 found by `find docker
    // scripts -name '*.sh'` plus run.sh itself). Not a closed list -- a
    // future script only needs to raise this count.
    assert!(
        scripts.len() >= 95,
        "only {} script(s) found under docker/, scripts/, and run.sh; expected at least 95",
        scripts.len()
    );

    let mut violations = Vec::new();
    for (path, text) in &scripts {
        for violation in kill_by_name_of_qemu_violations(text) {
            violations.push(format!("{path}: {violation} (#829/#849)"));
        }
    }
    assert!(
        violations.is_empty(),
        "script(s) under docker/, scripts/, or run.sh kill a qemu-ish process or \
         container by name/pattern instead of an invocation-owned id, so the first of \
         two concurrent invocations (of this script, a peer script sharing the same \
         image, or any other script matching the same bare pattern) to finish can kill \
         the other's still-running process out from under it:\n{}",
        violations.join("\n")
    );
}

/// ANTI-VACUITY: each predicate fires on the real shapes it claims to
/// (positive) and stays quiet on the real safe shapes already in the tree
/// post-fix (negative) -- both checked against literal strings first, then
/// against real file text via the mutation proofs below.
#[test]
fn kill_by_name_predicates_are_not_vacuous() {
    // --- Positive: each of the three shapes, as literal strings. ---
    assert!(
        line_is_pkill_or_killall_by_name(
            r#"pkill -9 -f "qemu-system-aarch64.*kernel-aarch64" 2>/dev/null || true"#
        ),
        "must detect a pattern-based pkill naming qemu"
    );
    assert!(
        line_is_pkill_or_killall_by_name("killall -9 qemu-system-x86_64 >/dev/null 2>&1 || true"),
        "must detect a pattern-based killall naming qemu"
    );
    assert!(
        line_is_pgrep_pipe_to_kill_by_name("kill $(pgrep -f qemu-system-aarch64) 2>/dev/null || true"),
        "must detect a kill fed from a pgrep search naming qemu"
    );
    assert!(
        line_is_docker_kill_or_stop_by_filter(
            "docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true",
            &HashSet::new(),
        ),
        "must detect an inline docker-ps-ancestor-filtered docker kill"
    );
    let mut existing_var = HashSet::new();
    existing_var.insert("EXISTING".to_string());
    assert!(
        line_is_docker_kill_or_stop_by_filter("    docker kill $EXISTING 2>/dev/null || true", &existing_var),
        "must detect a docker kill fed from a variable this file assigned from a \
         docker-ps-ancestor-filter query on an earlier line"
    );

    // --- Negative: the safe shapes this round's own fix introduces (and
    // the one that already existed pre-#829 in
    // docker/qemu-aarch64/run-arm64-boot.sh) must NOT trip any predicate.
    assert!(
        !line_is_docker_kill_or_stop_by_filter(r#"docker kill "$CONTAINER_NAME" 2>/dev/null || true"#, &HashSet::new()),
        "a kill of an invocation-minted container name must not be flagged"
    );
    assert!(
        !line_is_docker_kill_or_stop_by_filter(r#"    docker stop -t 5 "$CONTAINER_NAME" >/dev/null 2>&1 || true"#, &HashSet::new()),
        "a bounded-wait stop of an invocation-minted container name must not be flagged"
    );
    assert!(
        !line_is_pkill_or_killall_by_name(r#"    pkill -P "$pid" 2>/dev/null || true"#),
        "a -P (parent-PID) pkill must not be flagged even if it later gained a qemu mention"
    );
    assert!(
        !line_is_pkill_or_killall_by_name(
            "native=\"$( { pgrep -x qemu-system-aarch64 2>/dev/null || true; } | wc -l | tr -d ' ')\""
        ),
        "a pgrep-based host COUNT line (no kill verb at all) must not be flagged"
    );
    assert!(
        !line_is_pgrep_pipe_to_kill_by_name(
            "native=\"$( { pgrep -x qemu-system-aarch64 2>/dev/null || true; } | wc -l | tr -d ' ')\""
        ),
        "the lock helper's own pgrep COUNT line, which never feeds a kill, must not be flagged"
    );
    assert!(
        !line_is_pkill_or_killall_by_name(
            "# so cleanup below targets the exact container this run started instead of matching by ancestor image -- an ancestor-image filter can kill a DIFFERENT running container"
        ),
        "a doc comment describing the hazard in prose must not itself be flagged"
    );

    // --- Real-file sanity: each file fixed across #829 and #849 is clean
    // post-fix.
    let aarch64_fixed_paths = [
        "docker/qemu/run-aarch64-test.sh",
        "docker/qemu/run-aarch64-userspace.sh",
        "docker/qemu/run-aarch64-interactive.sh",
        "scripts/run-arm64-boot-test.sh",
    ];
    let x86_fixed_paths = [
        "docker/qemu/run-boot-parallel.sh",
        "docker/qemu/run-kthread-test.sh",
        "docker/qemu/run-kthread-parallel.sh",
        "docker/qemu/run-dns-test.sh",
        "docker/qemu/run-blocking-recv-test.sh",
        "docker/qemu/run-nonblock-eagain-test.sh",
        "docker/qemu/run-interactive.sh",
        "scripts/test-workqueue.sh",
        "scripts/f21-bisect-verdict.sh",
        "scripts/ci/ring3_check.sh",
    ];
    for path in aarch64_fixed_paths {
        let text = repo_text(path);
        assert!(
            sources_qemu_host_lock(&text),
            "sanity: {path} must still source the host lock"
        );
        let violations = kill_by_name_of_qemu_violations(&text);
        assert!(
            violations.is_empty(),
            "{path} must be clean post-fix, found: {violations:?}"
        );
    }
    for path in x86_fixed_paths {
        let text = repo_text(path);
        let violations = kill_by_name_of_qemu_violations(&text);
        assert!(
            violations.is_empty(),
            "{path} must be clean post-fix, found: {violations:?}"
        );
    }

    // --- ANTI-VACUITY mutation: the ratchet must redden against the real,
    // literal pre-fix text of each fixed file, not only synthetic strings.
    // Each reconstructed line/block below is copied verbatim from the
    // real pre-fix file text (the #829 issue body/PR diff for the aarch64
    // four, this file's own git history for the x86 ten), spliced back
    // into the real POST-fix file text read from disk.
    let pre_fix_test_sh_line =
        "docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true\n";
    let mutated_test_sh = format!("{}{}", repo_text("docker/qemu/run-aarch64-test.sh"), pre_fix_test_sh_line);
    assert!(
        !kill_by_name_of_qemu_violations(&mutated_test_sh).is_empty(),
        "reddening: reintroducing run-aarch64-test.sh's real pre-#829 cleanup line must trip the ratchet"
    );

    let pre_fix_userspace_sh_line =
        "docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true\n";
    let mutated_userspace_sh = format!(
        "{}{}",
        repo_text("docker/qemu/run-aarch64-userspace.sh"),
        pre_fix_userspace_sh_line
    );
    assert!(
        !kill_by_name_of_qemu_violations(&mutated_userspace_sh).is_empty(),
        "reddening: reintroducing run-aarch64-userspace.sh's real pre-#829 cleanup line must trip the ratchet"
    );

    let pre_fix_interactive_sh_block = "EXISTING=$(docker ps -q --filter ancestor=\"$IMAGE_NAME\" 2>/dev/null)\n\
if [ -n \"$EXISTING\" ]; then\n    \
    echo \"Stopping existing ARM64 containers...\"\n    \
    docker kill $EXISTING 2>/dev/null || true\nfi\n";
    let mutated_interactive_sh = format!(
        "{}{}",
        repo_text("docker/qemu/run-aarch64-interactive.sh"),
        pre_fix_interactive_sh_block
    );
    assert!(
        !kill_by_name_of_qemu_violations(&mutated_interactive_sh).is_empty(),
        "reddening: reintroducing run-aarch64-interactive.sh's real pre-#829 EXISTING/docker-kill \
         block (a two-line dataflow: the ancestor-filter assignment and the kill of that variable \
         on a later line) must trip the ratchet"
    );

    let pre_fix_boot_test_sh_line = "pkill -9 -f \"qemu-system-aarch64.*kernel-aarch64\" 2>/dev/null || true\n";
    let mutated_boot_test_sh = format!(
        "{}{}",
        repo_text("scripts/run-arm64-boot-test.sh"),
        pre_fix_boot_test_sh_line
    );
    assert!(
        !kill_by_name_of_qemu_violations(&mutated_boot_test_sh).is_empty(),
        "reddening: reintroducing run-arm64-boot-test.sh's real pre-#829 pattern-based pkill \
         line must trip the ratchet"
    );

    // #849's own ten x86-side files, each reddened with its real pre-fix
    // line(s).
    let x86_pre_fix_lines: [(&str, &[&str]); 9] = [
        (
            "docker/qemu/run-boot-parallel.sh",
            &["docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true\n"],
        ),
        (
            "docker/qemu/run-kthread-test.sh",
            &["docker kill $(docker ps -q --filter ancestor=\"$IMAGE_NAME\") 2>/dev/null || true\n"],
        ),
        (
            "docker/qemu/run-kthread-parallel.sh",
            &["docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true\n"],
        ),
        (
            "docker/qemu/run-dns-test.sh",
            &["docker kill $(docker ps -q --filter ancestor=\"$IMAGE_NAME\") 2>/dev/null || true\n"],
        ),
        (
            "docker/qemu/run-blocking-recv-test.sh",
            &["docker kill $(docker ps -q --filter ancestor=\"$IMAGE_NAME\") 2>/dev/null || true\n"],
        ),
        (
            "docker/qemu/run-nonblock-eagain-test.sh",
            &["docker kill $(docker ps -q --filter ancestor=\"$IMAGE_NAME\") 2>/dev/null || true\n"],
        ),
        (
            "scripts/test-workqueue.sh",
            &[
                "pkill -9 qemu-system-x86_64 2>/dev/null || true\n",
                "docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true\n",
            ],
        ),
        (
            "scripts/f21-bisect-verdict.sh",
            &[
                "pkill -9 qemu-system-x86 >/dev/null 2>&1 || true\n",
                "killall -9 qemu-system-x86_64 >/dev/null 2>&1 || true\n",
            ],
        ),
        (
            "scripts/ci/ring3_check.sh",
            &["pkill -f qemu-system-x86_64 >/dev/null 2>&1 || true\n"],
        ),
    ];
    for (path, pre_fix_lines) in x86_pre_fix_lines {
        let mut mutated = repo_text(path);
        for line in pre_fix_lines {
            mutated.push_str(line);
        }
        assert!(
            !kill_by_name_of_qemu_violations(&mutated).is_empty(),
            "reddening: reintroducing {path}'s real pre-#849 kill-by-name line(s) must trip the ratchet"
        );
    }

    // run-interactive.sh's pre-#849 shape was the same two-line
    // ancestor-filter/EXISTING dataflow as run-aarch64-interactive.sh's own
    // pre-#829 block, just against a different literal image name.
    let pre_fix_run_interactive_block = "EXISTING=$(docker ps -q --filter ancestor=\"$IMAGE_NAME\" 2>/dev/null)\n\
if [ -n \"$EXISTING\" ]; then\n    \
    echo \"Stopping existing breenix-qemu containers...\"\n    \
    docker kill $EXISTING 2>/dev/null || true\nfi\n";
    let mutated_run_interactive = format!(
        "{}{}",
        repo_text("docker/qemu/run-interactive.sh"),
        pre_fix_run_interactive_block
    );
    assert!(
        !kill_by_name_of_qemu_violations(&mutated_run_interactive).is_empty(),
        "reddening: reintroducing run-interactive.sh's real pre-#849 EXISTING/docker-kill \
         block must trip the ratchet"
    );

    // --- ANTI-VACUITY reach check: every fixed file is actually in this
    // ratchet's own census, not merely clean by having been skipped by the
    // census walk.
    // claim-lint:ok: #849
    let scripts = all_shell_scripts();
    let cooperators = lock_cooperator_scripts();
    for path in aarch64_fixed_paths {
        assert!(
            cooperators.iter().any(|(p, _)| p == path),
            "{path} must be walked by the aarch64 lock-cooperator census, or its clean \
             post-fix text proves nothing about detection"
        );
    }
    for path in aarch64_fixed_paths.iter().chain(x86_fixed_paths.iter()).chain(["docker/qemu/run-interactive.sh"].iter())
    {
        assert!(
            scripts.iter().any(|(p, _)| p == path),
            "{path} must be walked by this ratchet's own full-tree census, or its clean \
             post-fix text proves nothing about detection"
        );
    }
}
