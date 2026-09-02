//! Widening detector for `docs/planning/green-program/WORKLOAD-ENVELOPES.md` (R4).
//!
//! The green program's one revert (Filesystem x86/aarch64/blended, root cause #728)
//! happened because Filesystem was declared green against an x86 image that had
//! never run a second concurrent userspace process; an unrelated arc (#713) later
//! gave x86 that second process and a latent lock-discipline race became reachable.
//! No filesystem code changed between the declaration and the downgrade -- the
//! *workload* the declaration was proven under simply widened, and nothing recorded
//! what that workload was.
//!
//! This suite is the mechanical half of that fix. Items 1-4 read four workload axes
//! `WORKLOAD-ENVELOPES.md` documents for the program's six currently-standing green
//! cells (TTY x86/aarch64/blended, Tracing-aarch64, Bus-aarch64, NIC-aarch64) off
//! `init.rs`'s own call-site text, gate scripts' own QEMU flags, and a
//! kernel-registry function body -- but a round-2 review of this suite proved,
//! numerically, that #713's *actual* mechanism (a kernel dispatch-table capability
//! change, `kernel/src/syscall/handler.rs` commit a60b8855, with zero `init.rs` text
//! delta) is invisible to all four: a census of `init.rs` at the #713 merge scores
//! identically before and after. Item 5 closes that gap on the axis that actually
//! moved -- which `SyscallNumber` variants dispatch to a real handler versus an
//! ENOSYS stub, per arch -- and is the one item this suite can show, by replaying
//! the real pre/post-#713 `handler.rs` bytes through its own census logic, would
//! have caught #713's specific commit (see item 5's own comment block below for
//! that verification). Items 1-4 remain real, useful ratchets on the axes they DO
//! read; they are not claimed to cover #713's shape any more.
//!
//! Every check is a CENSUS in the sense every other `tests/*_structure.rs` ratchet
//! in this tree is: the *extraction* reads a fact out of the real source file it
//! governs, never string-matching against a copy of the envelope document's prose.
//! The *expectation* each extracted fact is compared against, however, IS a
//! hand-typed literal in several of the items below (item 1's `1`/`0` persistent
//! counts, item 3's `4`/`1` -smp values) -- these are this document's own numbers,
//! pinned here as a ratchet the same way this repo's other `*_structure.rs` files
//! pin literals for facts that are expected to stay fixed. That is a legitimate,
//! useful design (a pinned census still catches drift the moment source text
//! changes shape), just not what "never a hand-typed expected value" claimed.
//!
//! A change that widens one of these axes reddens the corresponding test by name
//! instead of silently invalidating a declaration nobody re-checks.
//!
//! It is host-side and requires no kernel build or QEMU boot: everything below is a
//! text read of files already in the tree. Run it the same way any other structural
//! ratchet here is run: `cargo test --test green_program_envelope_structure`.
//!
//! Every mutation-proof test below follows the established idiom in this file
//! family (see `tty_oracle_structure.rs`): mutations are applied to an in-memory
//! copy of the file content via `String::replace`, never written to disk, so a
//! mutation proof never risks leaving the tree dirty.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const INIT_RS: &str = "userspace/programs/src/init.rs";
const EXECUTOR_RS: &str = "kernel/src/test_framework/executor.rs";

/// The seven gate scripts `WORKLOAD-ENVELOPES.md` cites as backing the six
/// currently-standing green cells (2 x86, 5 aarch64 -- the tracing harness counts
/// once even though it drives both arches, since only its aarch64 branch backs a
/// standing cell).
const X86_GATE_SCRIPTS: &[&str] = &[
    "docker/qemu/run-x86-tty-oracle-gate.sh",
    "docker/qemu/run-x86-prod-profile-boot-test.sh",
];
const AARCH64_GATE_SCRIPTS: &[&str] = &[
    "docker/qemu/run-aarch64-tty-oracle-gate.sh",
    "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
    "docker/qemu/run-aarch64-full-test.sh",
    "docker/qemu/run-aarch64-service-sequence-gate.sh",
    "scripts/test_tracing_via_gdb.sh",
];

// ---------------------------------------------------------------------------
// Item 1: the TTY concurrency invariant (init.rs call-sequence census)
// ---------------------------------------------------------------------------

/// The `#[cfg(target_arch = "...")]`-gated call sequence inside `fn main() { ... }`,
/// in source order, as `(gate_arch, fn_name)` pairs. `gate_arch` is `None` for a
/// call with no `#[cfg(target_arch = ...)]` immediately above it. Only direct
/// `ident(...);` statement lines are recorded; truncates at the reap-loop comment
/// so the zombie-reap `loop { waitpid(-1, ...) }` below it (which is not part of
/// the launch sequence) is never walked.
fn main_call_sequence(source: &str) -> Vec<(Option<String>, String)> {
    let start = source.find("fn main() {").expect("init.rs declares fn main()");
    let after_header = start + "fn main() {".len();
    let reap_marker = "// Reap zombies forever";
    let reap_idx = source[after_header..]
        .find(reap_marker)
        .expect("init.rs main() reaches the reap-loop comment");
    let body = &source[after_header..after_header + reap_idx];

    let mut calls = Vec::new();
    let mut pending_cfg: Option<String> = None;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#[cfg(target_arch = \"") {
            let arch = rest
                .split('"')
                .next()
                .expect("cfg(target_arch = \"...\") names an arch")
                .to_string();
            pending_cfg = Some(arch);
            continue;
        }
        if let Some(paren) = line.find('(') {
            let candidate = &line[..paren];
            let is_bare_ident = !candidate.is_empty()
                && candidate
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            if is_bare_ident && line.trim_end().ends_with(");") {
                calls.push((pending_cfg.take(), candidate.to_string()));
                continue;
            }
        }
        // Any other statement (e.g. the `let pid = ...;` / `print!(...)` setup
        // lines at the top of main()) is not a call in the launch sequence; clear
        // a stray pending cfg defensively rather than mis-attach it to whatever
        // call comes next.
        pending_cfg = None;
    }
    calls
}

/// The body of `fn <name>(...) { ... }`, brace-matched from the first occurrence of
/// `fn <name>(` in `source`. Panics if the function is not declared -- a missing
/// function is exactly the kind of drift this census must not pass through
/// silently.
fn fn_body<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let idx = source
        .find(&needle)
        .unwrap_or_else(|| panic!("{INIT_RS} declares fn {name}(...)"));
    let after = &source[idx..];
    let open = after
        .find('{')
        .unwrap_or_else(|| panic!("fn {name} has a body"));
    let bytes = after.as_bytes();
    let mut depth: i32 = 1;
    let mut i = open + 1;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    &after[open + 1..i - 1]
}

#[derive(Debug, PartialEq, Eq)]
enum LaunchKind {
    /// Body calls neither `spawn(` nor `spawnv(` -- not a process launcher at all
    /// (e.g. `run_init_group_refusal_probe`, which drives a raw clone via inline
    /// asm, never `spawn()`).
    NotASpawn,
    /// Body calls `spawn(`/`spawnv(` and also `waitpid(` -- the child is reaped
    /// before the launcher returns, so it does not overlap with whatever main()
    /// calls next.
    Reaped,
    /// Body calls `spawn(`/`spawnv(` with no `waitpid(` anywhere in the same
    /// body -- a fire-and-forget launch that keeps running concurrently with
    /// everything main() calls afterward.
    Persistent,
}

fn classify_launcher(body: &str) -> LaunchKind {
    let has_spawn = body.contains("spawn(") || body.contains("spawnv(");
    let has_wait = body.contains("waitpid(");
    match (has_spawn, has_wait) {
        (false, _) => LaunchKind::NotASpawn,
        (true, true) => LaunchKind::Reaped,
        (true, false) => LaunchKind::Persistent,
    }
}

/// Counts persistent (fire-and-forget) launchers `main()` calls, for the given
/// `arch` ("aarch64" or "x86_64"), strictly before the first call to `stop_fn` that
/// is itself gated to `arch`. Only calls that actually execute on `arch` --
/// unconditional (`cfg == None`) or gated to `arch` itself -- are counted; calls
/// gated to the other arch are skipped, matching what the compiler would actually
/// build for that target.
fn persistent_count_before(source: &str, arch: &str, stop_fn: &str) -> usize {
    let calls = main_call_sequence(source);
    let mut count = 0;
    let mut found_stop = false;
    for (cfg, name) in &calls {
        if name == stop_fn && cfg.as_deref() == Some(arch) {
            found_stop = true;
            break;
        }
        let applies_to_arch = match cfg {
            None => true,
            Some(c) => c == arch,
        };
        if !applies_to_arch {
            continue;
        }
        if classify_launcher(fn_body(source, name)) == LaunchKind::Persistent {
            count += 1;
        }
    }
    assert!(
        found_stop,
        "never reached a {arch}-gated call to {stop_fn}(); main()'s call sequence \
         changed shape enough that this census can no longer find its own stop marker"
    );
    count
}

#[test]
fn aarch64_tty_oracle_runs_with_exactly_one_persistent_background_process() {
    let source = read(INIT_RS);
    let count = persistent_count_before(&source, "aarch64", "run_tty_oracle");
    assert_eq!(
        count, 1,
        "WORKLOAD-ENVELOPES.md \u{a7}1 declares aarch64's TTY oracle runs with exactly \
         one persistent background process (heartbeat) alive alongside it; the tree \
         now starts {count} before run_tty_oracle() is called on aarch64 -- the TTY \
         cell's proven concurrency envelope no longer matches the tree. Re-derive and \
         re-prove TTY-aarch64 (and re-check whether the new process can race an ext2 \
         write the same way #713 did for Filesystem) before trusting this declaration."
    );
}

#[test]
fn x86_tty_oracle_runs_with_no_persistent_background_process() {
    let source = read(INIT_RS);
    let count = persistent_count_before(&source, "x86_64", "run_tty_oracle");
    assert_eq!(
        count, 0,
        "WORKLOAD-ENVELOPES.md \u{a7}2 declares x86's TTY oracle runs with no \
         background daemon alive yet (bsshd/heartbeat-equivalent both start later); \
         the tree now starts {count} before run_tty_oracle() is called on x86 -- the \
         TTY-x86/blended concurrency envelope no longer matches the tree."
    );
}

// ---------------------------------------------------------------------------
// Item 2: ext2 read-only (x86) vs writable (aarch64) census
// ---------------------------------------------------------------------------

/// Whether the ext2 root-disk `-drive` (or, on the service-sequence gate, the
/// `drive_opts=` variable that becomes one) declares `readonly=on`. Scans line by
/// line rather than the whole file so a `readonly=on` on an unrelated drive (the
/// UEFI `id=hd` disk, x86's `id=placeholder`) can never be mistaken for the ext2
/// disk's own flag.
fn ext2_drive_is_readonly(script: &str) -> bool {
    for line in script.lines() {
        let l = line.trim();
        let names_ext2_drive = l.contains("id=ext2disk") || l.contains("id=ext2,");
        if names_ext2_drive && (l.contains("-drive") || l.starts_with("drive_opts=")) {
            return l.contains("readonly=on");
        }
    }
    panic!("no ext2 -drive (or drive_opts=) declaration found in this script");
}

#[test]
fn x86_gate_scripts_mount_ext2_readonly() {
    for path in X86_GATE_SCRIPTS {
        let script = read(path);
        assert!(
            ext2_drive_is_readonly(&script),
            "WORKLOAD-ENVELOPES.md \u{a7}2 and the Cross-cell structural facts section \
             both rely on {path} mounting the ext2 root disk `readonly=on` -- so any \
             ext2 write on this workload cannot persist (a bound on persistence, NOT \
             on whether #728's own lock-upgrade spin is reachable -- root_fs_write() \
             is a lock acquisition that succeeds independently of this flag). That \
             flag is now absent. This is exactly the shape of widening #728 needed: an \
             x86 filesystem-write envelope bound just weakened. Re-derive the TTY-x86 \
             and TTY-blended filesystem-envelope claims before trusting them."
        );
    }
}

#[test]
fn aarch64_gate_scripts_mount_ext2_writable() {
    for path in AARCH64_GATE_SCRIPTS {
        let script = read(path);
        assert!(
            !ext2_drive_is_readonly(&script),
            "WORKLOAD-ENVELOPES.md records every aarch64 gate in this document \
             mounting ext2 writable (no device-level guarantee against writes, unlike \
             x86); {path} now mounts it readonly=on. This narrows rather than widens \
             the envelope, but the document's own claim about this script is now \
             stale either way -- update WORKLOAD-ENVELOPES.md to match."
        );
    }
}

// ---------------------------------------------------------------------------
// Item 3: -smp census
// ---------------------------------------------------------------------------

/// The `-smp N` value on the same QEMU-invocation line as `anchor` (the flag that
/// names this line as the aarch64 or x86 launch, e.g. `-M virt,gic-version=3` or
/// `-machine pc,accel=tcg`) -- so a multi-arch script (the tracing harness) is read
/// correctly by picking the line for the arch actually being checked, not just the
/// first `-smp` in the file.
fn smp_on_line_containing(script: &str, anchor: &str) -> u32 {
    let line = script
        .lines()
        .find(|l| l.contains(anchor))
        .unwrap_or_else(|| panic!("no line containing `{anchor}` found in this script"));
    let idx = line
        .find("-smp ")
        .unwrap_or_else(|| panic!("no `-smp` flag on the `{anchor}` line: {line}"));
    let rest = &line[idx + "-smp ".len()..];
    let token = rest
        .split(|c: char| c.is_whitespace() || c == '\\')
        .find(|t| !t.is_empty())
        .expect("a value follows -smp");
    token
        .parse::<u32>()
        .unwrap_or_else(|_| panic!("-smp value `{token}` is not a number"))
}

const AARCH64_SMP_ANCHOR: &str = "-M virt,gic-version=3";
const X86_SMP_ANCHOR: &str = "-machine pc,accel=tcg";

#[test]
fn aarch64_gate_scripts_boot_smp_4() {
    for path in AARCH64_GATE_SCRIPTS {
        let script = read(path);
        let smp = smp_on_line_containing(&script, AARCH64_SMP_ANCHOR);
        assert_eq!(
            smp, 4,
            "WORKLOAD-ENVELOPES.md records every aarch64 gate backing a standing \
             cell booting -smp 4; {path} now boots -smp {smp}. CPU count is a \
             concurrency axis in its own right -- re-derive the affected cell's \
             envelope before trusting it."
        );
    }
}

#[test]
fn x86_gate_scripts_boot_smp_1() {
    for path in X86_GATE_SCRIPTS {
        let script = read(path);
        let smp = smp_on_line_containing(&script, X86_SMP_ANCHOR);
        assert_eq!(
            smp, 1,
            "WORKLOAD-ENVELOPES.md \u{a7}2 records both x86 TTY gates booting -smp 1 \
             (single CPU); {path} now boots -smp {smp}. This is precisely the shape \
             of axis the #728 postmortem says was never recorded for Filesystem --\
             re-derive TTY-x86's envelope before trusting it under the new CPU count."
        );
    }
}

// ---------------------------------------------------------------------------
// Item 4: the boot_tests registry stays kthread-only
// ---------------------------------------------------------------------------

#[test]
fn boot_tests_registry_stays_kthread_only() {
    let source = read(EXECUTOR_RS);
    let body = fn_body(&source, "run_all_tests");
    assert!(
        !body.contains("create_user_process"),
        "WORKLOAD-ENVELOPES.md \u{a7}4/\u{a7}5-6 rely on run_all_tests() (the \
         boot_tests-profile 109-test registry backing Tracing-aarch64 and \
         Bus/NIC-aarch64) never creating a real userspace process -- it now calls \
         create_user_process(). The Tracing/Bus/NIC aarch64 concurrency envelope was \
         proven against a kthread-only registry; re-derive it now that the registry \
         itself can spawn userspace processes."
    );
    assert!(
        !body.contains("spawn("),
        "run_all_tests() now calls spawn(); re-derive the Tracing/Bus/NIC aarch64 \
         concurrency envelope in WORKLOAD-ENVELOPES.md \u{a7}4/\u{a7}5-6."
    );
}

// ---------------------------------------------------------------------------
// Item 5: syscall-dispatch census -- which SyscallNumber variants dispatch to a
// real handler versus an ENOSYS stub, per arch. Added in the R4 fix round after
// review found items 1-4 blind to the actual #713 widening: #713's real change was
// commit a60b8855, a one-line kernel/src/syscall/handler.rs dispatch-table edit
// (`Some(SyscallNumber::Spawn) => SyscallResult::Err(NoSys)` became
// `=> super::handlers::sys_spawn(args.0, args.1)`) that no init.rs-reading census
// can ever see: init.rs's own call sites for start_bsshd()/run_boot_script()
// already existed as TEXT before #713 (their spawns just returned ENOSYS at
// runtime) -- only kernel *capability* changed, not any userspace call-site text.
// This is the axis the FS envelope was actually proven against and actually lost:
// a text census of init.rs at the #713 merge scores identically before and after
// (both 1 persistent x86 launcher, `start_bsshd`) even though the real world went
// from zero other processes ever having executed to `run_boot_script()`'s full
// process chain (#722) actually running.
//
// Verified against real history, not just asserted: this exact census logic, run
// standalone (a scratch `rustc --edition 2021` harness against two `git show`
// blobs, not committed here since embedding git archaeology into a source-text
// ratchet would make it fragile to history rewrites) against
// `kernel/src/syscall/handler.rs` at `09ae3f44^` (parent of the #713 merge, PR
// #730) and at `09ae3f44` (the merge itself), finds 126 named arms on both sides,
// all 125 non-Spawn arms byte-identical, and Spawn flipping from an ENOSYS stub to
// a real handler dispatch -- the census the mutation proof below reproduces
// generically DOES change across exactly the commit that mattered. Recorded in
// docs/planning/green-program/WORKLOAD-ENVELOPES.md's Detector section and in the
// R4 fix round's build notes.
// ---------------------------------------------------------------------------

/// Brace-matches from just after an already-located opening `{` (`after_open_brace`
/// is the byte index of the character following it) to its matching `}`, returning
/// the interior text. Same depth-counting loop as `fn_body` above, factored out
/// separately here because item 5 brace-matches from an anchor string inside a
/// function rather than from `fn name(` itself.
fn brace_match_from(source: &str, after_open_brace: usize) -> &str {
    let bytes = source.as_bytes();
    let mut depth: i32 = 1;
    let mut i = after_open_brace;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    &source[after_open_brace..i - 1]
}

/// Splits the interior of a `match { ... }` block into `(pattern, body)` pairs, one
/// per arm, tracking paren/brace/bracket depth together (sufficient for well-formed
/// Rust) and treating the first depth-0 `=>` after an arm boundary as the
/// pattern/body split. A `{ ... }`-bodied arm is brace-matched; an expression-bodied
/// arm runs to the next depth-0 comma. Comment text between arms rides along as
/// part of the *next* arm's `pattern` string, which is harmless here since callers
/// only grep that string for `SyscallNumber::Name` substrings, never treat it as
/// Rust syntax.
fn split_match_arms(block: &str) -> Vec<(String, String)> {
    let bytes = block.as_bytes();
    let mut arms = Vec::new();
    let mut i = 0usize;
    let mut arm_start = 0usize;
    let mut depth: i32 = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth -= 1,
            b'=' if depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                let pattern = block[arm_start..i].trim().to_string();
                let mut j = i + 2;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let body_begin = j;
                if j < bytes.len() && bytes[j] == b'{' {
                    let mut bdepth: i32 = 1;
                    j += 1;
                    while j < bytes.len() && bdepth > 0 {
                        match bytes[j] {
                            b'{' => bdepth += 1,
                            b'}' => bdepth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                } else {
                    let mut bdepth: i32 = 0;
                    while j < bytes.len() {
                        match bytes[j] {
                            b'(' | b'{' | b'[' => bdepth += 1,
                            b')' | b'}' | b']' => bdepth -= 1,
                            b',' if bdepth == 0 => break,
                            _ => {}
                        }
                        j += 1;
                    }
                }
                let body = block[body_begin..j].trim().to_string();
                arms.push((pattern, body));
                i = j;
                if i < bytes.len() && bytes[i] == b',' {
                    i += 1;
                }
                arm_start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    arms
}

/// The `SyscallNumber::Name` identifiers named in a match-arm pattern -- handles a
/// single variant (`Some(SyscallNumber::Exit)`), a bare variant (aarch64's
/// `SyscallNumber::Exit`), and an or-pattern (`SyscallNumber::Fork | SyscallNumber::Exec`).
/// Returns an empty vec for catch-all arms (`None`, or aarch64's `Some(syscall)`
/// binding with no literal variant name) -- those name no specific syscall and are
/// correctly excluded from the per-name census.
fn syscall_names_in_pattern(pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    let marker = "SyscallNumber::";
    let mut rest = pattern;
    while let Some(idx) = rest.find(marker) {
        let after = &rest[idx + marker.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        names.push(after[..end].to_string());
        rest = &after[end..];
    }
    names
}

/// Whether an arm's body is a hardcoded "this syscall is not implemented" stub --
/// the kernel textually admitting ENOSYS -- as opposed to a real dispatch to a
/// handler function, even a trivial always-fails one (Mremap's fixed ENOMEM is a
/// real, if minimal, handler and correctly does NOT count as a stub here: the
/// kernel admits the syscall, it just always fails it for a resource reason, which
/// is a different fact than "unimplemented").
fn is_enosys_stub(body: &str) -> bool {
    body.contains("NoSys") || body.contains("ENOSYS") || body.trim() == "(-38_i64) as u64"
}

const SYSCALL_HANDLER_RS: &str = "kernel/src/syscall/handler.rs";
const AARCH64_SYSCALL_ENTRY_RS: &str = "kernel/src/arch_impl/aarch64/syscall_entry.rs";

/// Syscall-dispatch census for the live x86_64 syscall path: `rust_syscall_handler`
/// in `kernel/src/syscall/handler.rs`, the Tier-1 hot-path handler wired to
/// `entry.asm` -- NOT `syscall/dispatcher.rs::dispatch_syscall`, which carries its
/// own `#[allow(dead_code)]` and is never called anywhere else in the tree
/// (checked: `grep -rn dispatch_syscall kernel/src` has no hits outside
/// dispatcher.rs itself -- it is genuinely dead code, not a second live path).
/// Returns `(variant_name, is_enosys_stub)` for every named `SyscallNumber`
/// variant in the table.
fn x86_syscall_census(source: &str) -> Vec<(String, bool)> {
    let anchor = "match SyscallNumber::from_u64(syscall_num) {";
    let start = source
        .find(anchor)
        .unwrap_or_else(|| {
            panic!("{SYSCALL_HANDLER_RS} dispatches on SyscallNumber::from_u64(syscall_num)")
        })
        + anchor.len();
    let block = brace_match_from(source, start);
    let mut out = Vec::new();
    for (pattern, body) in split_match_arms(block) {
        for name in syscall_names_in_pattern(&pattern) {
            out.push((name, is_enosys_stub(&body)));
        }
    }
    out
}

/// Syscall-dispatch census for the live aarch64 syscall path. Two match blocks feed
/// it, and BOTH must be read to get a truthful answer: `rust_syscall_handler_aarch64`
/// intercepts Fork/Exec/Sigreturn/Pause/Sigsuspend/Clone directly (they need
/// exception-frame access `dispatch_syscall_enum` doesn't have) before anything
/// reaches `dispatch_syscall_enum`; `dispatch_syscall_enum` itself still carries a
/// defensive `(-38_i64) as u64` arm for those same five names, per its own comment
/// "If they somehow reach here, return ENOSYS" -- text that is never actually
/// reached at runtime, since the outer match already intercepted them. A census
/// that read only `dispatch_syscall_enum` would misclassify five live syscalls as
/// ENOSYS stubs. This function reads the outer match first and lets it win;
/// `dispatch_syscall_enum`'s arms only contribute for names the outer match
/// doesn't already name.
fn aarch64_syscall_census(source: &str) -> Vec<(String, bool)> {
    let outer_anchor = "let result = match resolved_num {";
    let outer_start = source
        .find(outer_anchor)
        .unwrap_or_else(|| {
            panic!(
                "{AARCH64_SYSCALL_ENTRY_RS} dispatches on resolved_num in \
                 rust_syscall_handler_aarch64"
            )
        })
        + outer_anchor.len();
    let outer_block = brace_match_from(source, outer_start);

    let inner_fn_anchor = "fn dispatch_syscall_enum(";
    let inner_fn_start = source.find(inner_fn_anchor).unwrap_or_else(|| {
        panic!("{AARCH64_SYSCALL_ENTRY_RS} declares fn dispatch_syscall_enum(...)")
    });
    let inner_source = &source[inner_fn_start..];
    let match_anchor = "match syscall {";
    let inner_match_start = inner_source
        .find(match_anchor)
        .unwrap_or_else(|| panic!("dispatch_syscall_enum matches on syscall"))
        + match_anchor.len();
    let inner_block = brace_match_from(inner_source, inner_match_start);

    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (pattern, body) in split_match_arms(outer_block) {
        for name in syscall_names_in_pattern(&pattern) {
            if seen.insert(name.clone()) {
                out.push((name, is_enosys_stub(&body)));
            }
        }
    }
    for (pattern, body) in split_match_arms(inner_block) {
        for name in syscall_names_in_pattern(&pattern) {
            if seen.insert(name.clone()) {
                out.push((name, is_enosys_stub(&body)));
            }
        }
    }
    out
}

/// The names in `census` whose arm is an ENOSYS stub, sorted.
fn stub_names(census: &[(String, bool)]) -> Vec<String> {
    let mut names: Vec<String> = census
        .iter()
        .filter(|(_, is_stub)| *is_stub)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn x86_syscall_dispatch_enosys_stubs_are_exactly_gettime() {
    let source = read(SYSCALL_HANDLER_RS);
    let census = x86_syscall_census(&source);
    // Anti-vacuity anchor, same role as item 1's `found_stop`: a broken parser that
    // silently returns nothing must not pass this test by accident.
    assert!(
        census.iter().any(|(n, is_stub)| n == "Write" && !is_stub),
        "x86 syscall-dispatch census did not find Write as a live arm -- the parser \
         is not reading {SYSCALL_HANDLER_RS}'s real dispatch table"
    );
    let stubs = stub_names(&census);
    assert_eq!(
        stubs,
        vec!["GetTime".to_string()],
        "x86's ENOSYS-stub syscall set changed from [GetTime] to {stubs:?}. This is \
         the exact axis #713 widened on 2026-08-31 (Spawn moved ENOSYS -> \
         handlers::sys_spawn, kernel/src/syscall/handler.rs, commit a60b8855) and the \
         axis items 1-4 above are structurally blind to, because that widening never \
         touched userspace/programs/src/init.rs's call-site text. A syscall leaving \
         this set is new kernel-admitted capability on x86 -- re-derive every cell's \
         envelope that assumed it was unreachable before trusting them."
    );
}

#[test]
fn aarch64_syscall_dispatch_enosys_stubs_are_exactly_archprctl() {
    let source = read(AARCH64_SYSCALL_ENTRY_RS);
    let census = aarch64_syscall_census(&source);
    assert!(
        census.iter().any(|(n, is_stub)| n == "Write" && !is_stub),
        "aarch64 syscall-dispatch census did not find Write as a live arm -- the \
         parser is not reading {AARCH64_SYSCALL_ENTRY_RS}'s real dispatch table"
    );
    let stubs = stub_names(&census);
    assert_eq!(
        stubs,
        vec!["ArchPrctl".to_string()],
        "aarch64's ENOSYS-stub syscall set changed from [ArchPrctl] to {stubs:?}. A \
         syscall leaving this set is new kernel-admitted capability on aarch64 -- \
         re-derive every standing cell's envelope before trusting it."
    );
}

// ---------------------------------------------------------------------------
// Mutation proofs -- each item above, proven to redden on a #713-shaped widening
// and stay quiet on an unrelated change. All mutations are applied to in-memory
// copies only (String::replace), matching this file family's existing idiom; the
// tree on disk is never touched.
// ---------------------------------------------------------------------------

#[test]
fn item1_mutation_proof() {
    let source = read(INIT_RS);
    let baseline = persistent_count_before(&source, "aarch64", "run_tty_oracle");
    assert_eq!(baseline, 1, "sanity: baseline must match the live #[test] above");

    // WIDENING: insert a second persistent aarch64 spawn ahead of run_tty_oracle(),
    // the same shape of change #713 made to Filesystem (a new concurrent process
    // appears ahead of code that assumed it was alone).
    let widened = source.replace(
        "    #[cfg(target_arch = \"aarch64\")]\n    run_tty_oracle();",
        "    #[cfg(target_arch = \"aarch64\")]\n    start_bounce();\n    #[cfg(target_arch = \"aarch64\")]\n    run_tty_oracle();",
    );
    assert_ne!(widened, source, "mutation did not apply");
    let widened_count = persistent_count_before(&widened, "aarch64", "run_tty_oracle");
    assert_eq!(
        widened_count, 2,
        "inserting a second persistent aarch64 launcher ahead of run_tty_oracle() \
         did not change the census -- the check would not have caught a #713-shaped \
         widening"
    );
    assert_ne!(
        widened_count, baseline,
        "the widening must be detectable as a change from the live baseline"
    );

    // CONTROL: an unrelated edit inside an already-reaped launcher's own print
    // string. Must not move the census at all.
    let control = source.replace(
        "\"[init] futex_handoff_oracle exited pid={} code={}\\n\"",
        "\"[init] futex_handoff_oracle finished pid={} code={}\\n\"",
    );
    assert_ne!(control, source, "control mutation did not apply");
    let control_count = persistent_count_before(&control, "aarch64", "run_tty_oracle");
    assert_eq!(
        control_count, baseline,
        "an unrelated print-string edit inside a reaped launcher must not move the \
         persistent-launcher census"
    );

    // WIDENING (x86 arm): the same shape on x86's own count -- the arm that models
    // #713's *text* shape directly (M7: this arm previously had no mutation proof
    // of its own; the aarch64 case above exercises the same shared
    // persistent_count_before()/classify_launcher() logic, but not the x86-gated
    // read path specifically). A #713-shaped PR that added a *textual* fire-and-
    // forget spawn ahead of x86's run_tty_oracle() -- as opposed to #713's actual
    // mechanism, a dispatch-table capability change with no init.rs text delta at
    // all, see item 5 above -- would be caught here.
    let x86_baseline = persistent_count_before(&source, "x86_64", "run_tty_oracle");
    assert_eq!(x86_baseline, 0, "sanity: x86 baseline must match the live #[test] above");
    let x86_widened = source.replace(
        "    #[cfg(target_arch = \"x86_64\")]\n    run_tty_oracle();",
        "    #[cfg(target_arch = \"x86_64\")]\n    start_bounce();\n    #[cfg(target_arch = \"x86_64\")]\n    run_tty_oracle();",
    );
    assert_ne!(x86_widened, source, "x86 mutation did not apply");
    let x86_widened_count = persistent_count_before(&x86_widened, "x86_64", "run_tty_oracle");
    assert_eq!(
        x86_widened_count, 1,
        "inserting a persistent x86 launcher ahead of run_tty_oracle() did not \
         change the x86 census"
    );
}

#[test]
fn item2_mutation_proof() {
    let script = read("docker/qemu/run-x86-tty-oracle-gate.sh");
    assert!(ext2_drive_is_readonly(&script), "sanity: baseline must be readonly");

    // WIDENING: the x86 ext2 disk loses its readonly flag -- the device-level
    // persistence bound this document leans on (§2's corrected wording: a bound on
    // whether a write can persist, not on whether #728's lock path is reachable)
    // disappears.
    let widened = script.replace(
        "if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_IMG",
        "if=none,id=ext2disk,format=raw,file=$EXT2_IMG",
    );
    assert_ne!(widened, script, "mutation did not apply");
    assert!(
        !ext2_drive_is_readonly(&widened),
        "stripping readonly=on from the x86 ext2 drive line did not flip the check -- \
         the persistence bound could be lost silently"
    );

    // CONTROL: readonly=on stays, but an unrelated drive (the placeholder disk)
    // gets edited. Must not move the ext2 check either way.
    let control = script.replace(
        "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_ROOT/placeholder.img",
        "if=none,id=placeholder,format=raw,file=$OUTPUT_ROOT/placeholder.img",
    );
    assert_ne!(control, script, "control mutation did not apply");
    assert!(
        ext2_drive_is_readonly(&control),
        "editing an unrelated drive's readonly flag must not affect the ext2 census"
    );

    // WIDENING (aarch64 arm): M7 -- the aarch64-writable arm previously had no
    // mutation proof of its own. Flipping an aarch64 script to readonly=on is a
    // *narrowing* of the concurrency-relevant axis (a write that lands would now
    // fail to persist there too), but it is still the shape of drift
    // `aarch64_gate_scripts_mount_ext2_writable` exists to catch, and the doc says
    // as much: "this narrows rather than widens the envelope, but the document's
    // own claim about this script is now stale either way."
    let aarch64_script = read("docker/qemu/run-aarch64-tty-oracle-gate.sh");
    assert!(
        !ext2_drive_is_readonly(&aarch64_script),
        "sanity: aarch64 baseline must be writable"
    );
    let aarch64_widened = aarch64_script.replace(
        "-drive if=none,id=ext2,format=raw,file=\"$RUN_DIR/ext2-writable.img\"",
        "-drive if=none,id=ext2,format=raw,readonly=on,file=\"$RUN_DIR/ext2-writable.img\"",
    );
    assert_ne!(aarch64_widened, aarch64_script, "aarch64 mutation did not apply");
    assert!(
        ext2_drive_is_readonly(&aarch64_widened),
        "adding readonly=on to the aarch64 ext2 drive line did not flip the check"
    );
}

#[test]
fn item3_mutation_proof() {
    let script = read("docker/qemu/run-x86-tty-oracle-gate.sh");
    let baseline = smp_on_line_containing(&script, X86_SMP_ANCHOR);
    assert_eq!(baseline, 1, "sanity: baseline must match the live #[test] above");

    // WIDENING: x86 goes multi-CPU. A real concurrency-envelope change.
    let widened = script.replace(
        "-machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512",
        "-machine pc,accel=tcg -cpu qemu64 -smp 2 -m 512",
    );
    assert_ne!(widened, script, "mutation did not apply");
    assert_eq!(
        smp_on_line_containing(&widened, X86_SMP_ANCHOR),
        2,
        "bumping -smp on the x86 QEMU line did not change what the census reads"
    );

    // CONTROL: -smp changed on a real x86 gate script none of the six standing
    // cells cite (run-x86-boot-tests.sh -- confirmed below to be in neither watched
    // array, and confirmed to carry its own real `-smp 1` on the same anchor line
    // shape). This is a genuine mutation (assert_ne, and smp_on_line_containing's
    // own reading of THIS file's bytes really does change), fed through the exact
    // same parsing function the live #[test]s use -- not a no-op read. What makes
    // it a control rather than a second widening test is that the live #[test]s
    // (`x86_gate_scripts_boot_smp_1`, `aarch64_gate_scripts_boot_smp_4`) only ever
    // loop over X86_GATE_SCRIPTS/AARCH64_GATE_SCRIPTS, so this file's bytes --
    // mutated or not -- are structurally never read by either: re-reading the real
    // watched scripts from disk below (untouched, since every mutation in this file
    // family stays in-memory) still returns exactly the live baseline.
    let unrelated_path = "docker/qemu/run-x86-boot-tests.sh";
    assert!(
        !X86_GATE_SCRIPTS.contains(&unrelated_path) && !AARCH64_GATE_SCRIPTS.contains(&unrelated_path),
        "sanity: {unrelated_path} must not be one of the watched gate scripts, or \
         this control proves nothing"
    );
    let unrelated = read(unrelated_path);
    let unrelated_baseline = smp_on_line_containing(&unrelated, X86_SMP_ANCHOR);
    let unrelated_widened = unrelated.replace(
        "-machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512",
        "-machine pc,accel=tcg -cpu qemu64 -smp 2 -m 512",
    );
    assert_ne!(unrelated_widened, unrelated, "control mutation did not apply");
    assert_ne!(
        smp_on_line_containing(&unrelated_widened, X86_SMP_ANCHOR),
        unrelated_baseline,
        "the control script's own -smp value should have changed under its own \
         mutation -- if it didn't, this control is vacuous the same way the old one \
         was"
    );
    for path in X86_GATE_SCRIPTS {
        let watched_script = read(path);
        assert_eq!(
            smp_on_line_containing(&watched_script, X86_SMP_ANCHOR),
            1,
            "watched script {path}'s -smp reading moved after mutating an unrelated, \
             unwatched script -- the two censuses must be independent"
        );
    }

    // WIDENING (aarch64 arm): M7 -- the aarch64 -smp-4 arm previously had no
    // mutation proof of its own.
    let aarch64_script = read("docker/qemu/run-aarch64-tty-oracle-gate.sh");
    let aarch64_baseline = smp_on_line_containing(&aarch64_script, AARCH64_SMP_ANCHOR);
    assert_eq!(aarch64_baseline, 4, "sanity: aarch64 baseline must match the live #[test] above");
    let aarch64_widened = aarch64_script.replace(
        "-M virt,gic-version=3 -cpu max -m 512 -smp 4",
        "-M virt,gic-version=3 -cpu max -m 512 -smp 2",
    );
    assert_ne!(aarch64_widened, aarch64_script, "aarch64 mutation did not apply");
    assert_eq!(
        smp_on_line_containing(&aarch64_widened, AARCH64_SMP_ANCHOR),
        2,
        "dropping -smp on the aarch64 QEMU line did not change what the census reads"
    );
}

#[test]
fn item4_mutation_proof() {
    let source = read(EXECUTOR_RS);
    let baseline_body = fn_body(&source, "run_all_tests").to_string();
    assert!(
        !baseline_body.contains("create_user_process") && !baseline_body.contains("spawn("),
        "sanity: baseline run_all_tests() must be kthread-only"
    );

    // WIDENING: splice a real create_user_process() call into run_all_tests()'s own
    // SOURCE TEXT (not a standalone string built by this test), then re-extract the
    // function body via the SAME fn_body() the live #[test] above uses, and check
    // the same substring the live check watches. (Before this fix, this arm instead
    // built a string via format!(baseline_body, ..) and asserted the string it had
    // just concatenated contained what it had just concatenated -- never calling
    // fn_body() again or touching the check predicate. Proof that mattered:
    // sabotaging fn_body() to always return an empty slice left this test green.)
    let widened_source = source.replace(
        "    serial_println!(\"[BOOT_TESTS:START]\");",
        "    serial_println!(\"[BOOT_TESTS:START]\");\n    let _ = kernel::process::creation::create_user_process(alloc::string::String::new(), &[]);",
    );
    assert_ne!(widened_source, source, "mutation did not apply");
    let widened_body = fn_body(&widened_source, "run_all_tests");
    assert!(
        widened_body.contains("create_user_process"),
        "splicing a create_user_process() call into run_all_tests()'s source did not \
         show up when the function body was re-extracted by fn_body() -- the \
         mutation and the extractor disagree about where the function body is, so \
         the live check would not have caught a #713-shaped widening of the \
         boot_tests registry itself"
    );

    // Same shape, the other substring the live check watches (`spawn(`).
    let widened_source_spawn = source.replace(
        "    serial_println!(\"[BOOT_TESTS:START]\");",
        "    serial_println!(\"[BOOT_TESTS:START]\");\n    let _ = spawn(0, 0);",
    );
    assert_ne!(widened_source_spawn, source, "spawn( mutation did not apply");
    let widened_body_spawn = fn_body(&widened_source_spawn, "run_all_tests");
    assert!(
        widened_body_spawn.contains("spawn("),
        "splicing a spawn( call into run_all_tests()'s source did not show up when \
         the function body was re-extracted by fn_body()"
    );

    // CONTROL: a create_user_process-shaped call appears elsewhere in the same
    // file, outside run_all_tests()'s own body. The item-4 check only reads
    // run_all_tests()'s body, so this must not affect it.
    let control = source.replace(
        "/// Run all registered tests in parallel (EarlyBoot stage only)",
        "/// Run all registered tests in parallel (EarlyBoot stage only)\n/// (unrelated doc note: some other function might call create_user_process)",
    );
    assert_ne!(control, source, "control mutation did not apply");
    let control_body = fn_body(&control, "run_all_tests");
    assert!(
        !control_body.contains("create_user_process"),
        "an edit outside run_all_tests()'s own body must not leak into its census"
    );
}

#[test]
fn item5_mutation_proof() {
    // WIDENING: reproduce the #713 shape exactly -- an ENOSYS-stub arm becomes a
    // real dispatch. x86's GetTime is the one currently-stubbed arm available to
    // mutate without inventing a hypothetical (Spawn itself is already live
    // post-#713, so there is nothing left to widen there on the current tree).
    let source = read(SYSCALL_HANDLER_RS);
    let baseline = x86_syscall_census(&source);
    assert_eq!(
        stub_names(&baseline),
        vec!["GetTime".to_string()],
        "sanity: baseline must match the live #[test] above"
    );

    let widened = source.replace(
        "Some(SyscallNumber::GetTime) => SyscallResult::Err(super::ErrorCode::NoSys as u64),",
        "Some(SyscallNumber::GetTime) => super::handlers::sys_gettid(),",
    );
    assert_ne!(widened, source, "mutation did not apply");
    assert!(
        stub_names(&x86_syscall_census(&widened)).is_empty(),
        "flipping GetTime's arm from an ENOSYS stub to a real dispatch did not \
         change the census -- the check would not have caught a #713-shaped \
         widening"
    );

    // CONTROL: an unrelated already-live arm is rewritten to call a different
    // (still real) function. It stays live either way; the stub set must not move.
    let control = source.replace(
        "Some(SyscallNumber::Yield) => super::handlers::sys_yield(),",
        "Some(SyscallNumber::Yield) => super::handlers::sys_yield_now(),",
    );
    assert_ne!(control, source, "control mutation did not apply");
    assert_eq!(
        stub_names(&x86_syscall_census(&control)),
        vec!["GetTime".to_string()],
        "renaming an already-live arm's target function must not move the \
         ENOSYS-stub census"
    );

    // Same widening shape on aarch64, using its one stubbed arm (ArchPrctl) -- the
    // review's B1 finding asked for this axis to hold "for each arch," so both
    // arches get their own widening proof rather than just x86's.
    let aarch64_source = read(AARCH64_SYSCALL_ENTRY_RS);
    let aarch64_baseline = aarch64_syscall_census(&aarch64_source);
    assert_eq!(
        stub_names(&aarch64_baseline),
        vec!["ArchPrctl".to_string()],
        "sanity: aarch64 baseline must match the live #[test] above"
    );
    let aarch64_widened = aarch64_source.replace(
        "SyscallNumber::ArchPrctl => (-(crate::syscall::errno::ENOSYS as i64)) as u64,",
        "SyscallNumber::ArchPrctl => sys_gettid(),",
    );
    assert_ne!(aarch64_widened, aarch64_source, "aarch64 mutation did not apply");
    assert!(
        stub_names(&aarch64_syscall_census(&aarch64_widened)).is_empty(),
        "flipping aarch64's ArchPrctl arm from an ENOSYS stub to a real dispatch did \
         not change the census"
    );
}
