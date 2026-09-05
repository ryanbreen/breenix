//! Structural regressions for the aarch64 TTBR0 shadow-reconciliation
//! discipline (#786).
//!
//! The per-CPU words the syscall return corridor reads decide which page-table
//! root the next return to EL0 runs on. These checks pin the shape that keeps
//! each process-root install and the shadows describing it in agreement. They
//! are intentionally about behavior-bearing call shapes rather than line
//! numbers.
//!
//! Two different things are counted in this slice and they are not the same
//! number, so both are stated here rather than left to be inferred:
//!
//!   * 10 process-root install DECISION sites existed on `main` -- the places
//!     that chose a root and put it in TTBR0_EL1. 9 of the 10 are routed
//!     through `ttbr0::adopt_process_ttbr0` by this slice; the 10th is the
//!     Tier-1 site `kernel/src/syscall/time.rs::ensure_current_address_space`,
//!     which this branch may not touch.
//!   * 7 FUNCTIONS still write TTBR0_EL1 with a raw `msr` at this head, and
//!     those 7 are what the census below walks: 2 discipline-module helpers,
//!     2 that reconcile both shadows inline, 2 mechanism primitives that
//!     install what a caller decided, and the 1 Tier-1 site.
//!
//! The 9 routed sites are absent from the 7 precisely because they no longer
//! write the register themselves. Both accountings are enumerated in
//! docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
//! claim-lint:ok: 9 of 10 decision sites are routed at this head and the 10th
//! is the Tier-1 site, which
//! `every_ttbr0_install_settles_the_per_cpu_shadows` prints on each run.


use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// Every Rust source below `relative`, as (repo-relative path, contents).
/// claim-lint:ok: the walk is the same one `tests/teardown_structure.rs` uses
fn rust_sources_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("source below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, fs::read_to_string(path).expect("read Rust source")));
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join(relative), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}");
    let start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("find function {name}"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("find opening brace for {name}"));
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("find closing brace for {name}")
}
// ---------------------------------------------------------------------------
// TTBR0 install census (#786)
// ---------------------------------------------------------------------------
//
// The per-CPU words `saved_process_cr3` (offset 80) and `next_cr3` (offset 64)
// are not bookkeeping: the syscall return corridor in `syscall_entry.S`
// installs `next_cr3` when it is non-zero and otherwise restores
// `saved_process_cr3`, and `is_ttbr0_root_live_in_mask` reads both before a
// page-table root may be reclaimed. A site that writes TTBR0_EL1 with a raw
// `msr` and leaves those words naming a different root does not merely
// disagree with the register -- it decides which root the next return to EL0
// runs on.
// claim-lint:ok: the corridor reads are cited in
// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
//
// The census below walks the Rust functions under `kernel/src` that write
// TTBR0_EL1 -- its coverage floor is documented on `ttbr0_install_census` -- and
// sorts them by shape, not by a list of known sites:
//
//   * the discipline itself (`arch_impl/aarch64/ttbr0.rs`);
//   * functions that reconcile inline (they name `set_saved_process_cr3`);
//   * mechanism primitives, whose installed value traces back to one of their
//     own parameters -- they install what a caller decided, so the caller owns
//     the shadows;
//   * everything else, which must be empty outside the files CLAUDE.md lists
//     as Tier-1 prohibited.
// claim-lint:ok: 7 censused functions at this head, enumerated in
// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md

/// Files CLAUDE.md forbids modifying without explicit user approval. Checked
/// back against CLAUDE.md by `the_tier_one_exemption_matches_the_project_rule`
/// so this stays the project's rule rather than this test's opinion.
const TIER_ONE_PROHIBITED: [&str; 3] = [
    "kernel/src/syscall/handler.rs",
    "kernel/src/syscall/time.rs",
    "kernel/src/interrupts/timer.rs",
];

fn identifiers(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            out.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.insert(current);
    }
    out
}

/// The right-hand side of the first `let <name> = ...;` binding in `body`.
fn let_binding_rhs(body: &str, name: &str) -> Option<String> {
    for prefix in [format!("let {name} = "), format!("let mut {name} = ")] {
        if let Some(start) = body.find(&prefix) {
            let rest = &body[start + prefix.len()..];
            let end = rest.find(';').unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Whether `operand` traces back, through this function's own `let` bindings,
/// to something the function was handed. Depth-limited: a mechanism primitive
/// masks or tags a parameter at most a couple of steps before installing it.
fn traces_to_a_parameter(signature: &str, body: &str, operand: &str) -> bool {
    let params = identifiers(signature);
    let mut frontier = vec![operand.to_string()];
    let mut seen = BTreeSet::new();
    for _ in 0..4 {
        let mut next = Vec::new();
        for name in std::mem::take(&mut frontier) {
            if params.contains(&name) {
                return true;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(rhs) = let_binding_rhs(body, &name) {
                next.extend(identifiers(&rhs));
            }
        }
        if next.is_empty() {
            return false;
        }
        frontier = next;
    }
    false
}

/// Every function name declared in `source`, in declaration order.
/// claim-lint:ok: the coverage floor asserted by `ttbr0_install_census` is what
/// measures this, 7 of 7 at this head
fn declared_function_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let rest = match trimmed.split_once("fn ") {
            Some((before, rest)) if before.is_empty() || before.ends_with(' ') => rest,
            _ => continue,
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

struct TtbrInstall {
    file: String,
    function: String,
    signature: String,
    body: String,
    /// The `impl` type that owns this function, when it has one.
    owner: Option<String>,
}

/// The type whose `impl` block encloses `name`, if any: the call-site prefix a
/// caller has to spell. `write` alone is far too common a name to search for;
/// `Cr3::write` is the call this census is about.
fn enclosing_impl_type(source: &str, name: &str) -> Option<String> {
    let needle = format!("fn {name}");
    let lines: Vec<&str> = source.lines().collect();
    let declaration = lines.iter().position(|line| line.contains(&needle))?;
    for index in (0..declaration).rev() {
        let line = lines[index].trim_start();
        if !line.starts_with("impl") {
            continue;
        }
        let header = line.trim_end_matches('{').trim();
        let subject = match header.rsplit_once(" for ") {
            Some((_, target)) => target,
            None => header.trim_start_matches("impl").trim(),
        };
        let type_name: String = subject
            .trim()
            .chars()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect();
        if type_name.is_empty() {
            return None;
        }
        return Some(type_name);
    }
    None
}

/// The declaration line of `name` in `source`.
fn function_signature(source: &str, name: &str) -> String {
    let needle = format!("fn {name}");
    for line in source.lines() {
        if line.contains(&needle) {
            return line.to_string();
        }
    }
    panic!("find signature for {name}")
}

/// Every function under `kernel/src` whose body writes TTBR0_EL1.
/// The walk is name-driven, so what it carries is a coverage FLOOR: the install
/// occurrences inside censused bodies must be at least as many as the file
/// holds. Nested functions can be double-counted, so the floor does not by
/// itself exclude one hidden site paired with one double-count; what it does
/// catch is a file whose installs the name walk missed outright.
/// claim-lint:ok: 7 of 7 install-writing functions are censused at this head,
/// enumerated in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
fn ttbr0_install_census(sources: &[(String, String)]) -> Vec<TtbrInstall> {
    const INSTALL: &str = "msr ttbr0_el1";
    let mut out = Vec::new();
    for (file, source) in sources {
        let total = source.matches(INSTALL).count();
        if total == 0 {
            continue;
        }
        let mut accounted = 0usize;
        for name in declared_function_names(source) {
            let body = function_body(source, &name);
            let hits = body.matches(INSTALL).count();
            if hits == 0 {
                continue;
            }
            accounted = accounted.saturating_add(hits);
            out.push(TtbrInstall {
                file: file.clone(),
                function: name.clone(),
                signature: function_signature(source, &name),
                body: body.to_string(),
                owner: enclosing_impl_type(source, &name),
            });
        }
        assert!(
            accounted >= total,
            "{file}: the census reached {accounted} of {total} TTBR0 install occurrences, so at \
             least one is in no censused function"
        );
    }
    out
}

/// The identifier handed to the asm block as the value being installed.
fn install_operand(body: &str) -> String {
    match body.split_once("in(reg)") {
        Some((_, rest)) => rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect(),
        None => String::new(),
    }
}

/// The per-CPU TTBR0 shadow words and their accessors. A function that names
/// any of them is deciding TTBR0 policy, not performing a mechanical write, so
/// it cannot claim the mechanism-primitive exemption below.
const SHADOW_NAMES: [&str; 4] = [
    "set_saved_process_cr3",
    "set_next_cr3",
    "saved_process_cr3",
    "next_cr3",
];

/// Whether each step that produces `operand` is either a parameter or a method
/// call on something parameter-borne -- i.e. the value came in through the
/// signature rather than being fetched.
///
/// R7-002: `traces_to_a_parameter` alone accepts `let root = fetch_root(flags);`
/// because `flags` is a parameter, which would let a future process-root wrapper
/// call itself a primitive. A path-qualified call or a bare free call in the
/// derivation chain means the value came from somewhere the caller did not hand
/// over, and that is the shape the exemption must not cover.
fn derivation_fetches_nothing(body: &str, operand: &str) -> bool {
    let mut frontier = vec![operand.to_string()];
    let mut seen = BTreeSet::new();
    for _ in 0..4 {
        let mut next = Vec::new();
        for name in std::mem::take(&mut frontier) {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(rhs) = let_binding_rhs(body, &name) {
                if !expression_only_calls_methods(&rhs) {
                    return false;
                }
                next.extend(identifiers(&rhs));
            }
        }
        if next.is_empty() {
            return true;
        }
        frontier = next;
    }
    true
}

/// Whether each call in `expression` is a method call rather than a
/// path-qualified or free call.
fn expression_only_calls_methods(expression: &str) -> bool {
    let characters: Vec<char> = expression.chars().collect();
    for (index, character) in characters.iter().enumerate() {
        if *character != '(' {
            continue;
        }
        let mut start = index;
        while start > 0 && (characters[start - 1].is_alphanumeric() || characters[start - 1] == '_')
        {
            start -= 1;
        }
        if start == index {
            continue;
        }
        if start == 0 || characters[start - 1] != '.' {
            return false;
        }
    }
    true
}

/// A mechanism primitive installs what its caller decided and fetches no value
/// of its own, so the obligation to settle the shadows belongs to that caller -- an
/// obligation `every_aarch64_caller_of_a_mechanism_primitive_settles_the_shadows`
/// then checks, rather than leaving the exemption a free pass.
fn is_mechanism_primitive(signature: &str, body: &str, operand: &str) -> bool {
    if operand.is_empty() {
        return false;
    }
    if !traces_to_a_parameter(signature, body, operand) {
        return false;
    }
    if !derivation_fetches_nothing(body, operand) {
        return false;
    }
    !SHADOW_NAMES.iter().any(|name| body.contains(name))
}

/// The attribute block immediately above `name`'s declaration.
fn function_attributes(source: &str, name: &str) -> String {
    let needle = format!("fn {name}");
    let lines: Vec<&str> = source.lines().collect();
    let declaration = match lines.iter().position(|line| line.contains(&needle)) {
        Some(index) => index,
        None => return String::new(),
    };
    let mut attributes = Vec::new();
    let mut cursor = declaration;
    while cursor > 0 {
        let candidate = lines[cursor - 1].trim();
        if candidate.starts_with("#[") || candidate.starts_with("//") {
            attributes.push(candidate.to_string());
            cursor -= 1;
        } else {
            break;
        }
    }
    attributes.join("\n")
}

/// Functions this census can show are compiled on aarch64: those under
/// `kernel/src/arch_impl/aarch64/`, plus any function carrying
/// `#[cfg(target_arch = "aarch64")]`.
///
/// Disclosed narrowing: shared code with no `cfg` at all is NOT in scope, so
/// this census cannot speak for a primitive call added to a cfg-free shared
/// helper. `kernel/src/memory/kernel_page_table.rs::build_master_kernel_pml4`
/// is exactly that shape at this head: cfg-free, in a cfg-free file, and it
/// calls the `Cr3::write` primitive -- so neither this filter nor anything
/// built on it reaches it. Nothing on aarch64 executes it today, because its
/// only caller is the cfg-free `kernel/src/memory/mod.rs::init`, whose only
/// caller is `kernel_main` in `kernel/src/main.rs` behind
/// `#[cfg(target_arch = "x86_64")]`. That is a fact about the current call
/// graph, not something this filter checks: a cfg-free caller added on the
/// aarch64 side would sit outside each census in this file.
/// claim-lint:ok: 7 of 7 censused TTBR0 install sites follow one of the 2
/// conventions above (#786).
fn aarch64_scoped_functions(sources: &[(String, String)]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (file, source) in sources {
        let arch_file = file.contains("/aarch64/");
        for name in declared_function_names(source) {
            let attributes = function_attributes(source, &name);
            if !arch_file && !attributes.contains("target_arch = \"aarch64\"") {
                continue;
            }
            out.push((
                file.clone(),
                name.clone(),
                function_body(source, &name).to_string(),
            ));
        }
    }
    out
}

/// Whether `body` calls `name` other than as a method or in its own declaration.
fn calls_function(body: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    let mut cursor = 0usize;
    while let Some(offset) = body[cursor..].find(&needle) {
        let at = cursor + offset;
        let preceding = body[..at].chars().last();
        let is_method = preceding == Some('.');
        let is_declaration = body[..at].ends_with("fn ");
        let is_longer_identifier = preceding
            .map(|character| character.is_alphanumeric() || character == '_')
            .unwrap_or(false);
        if !is_method && !is_declaration && !is_longer_identifier {
            return true;
        }
        cursor = at + needle.len();
    }
    false
}

/// aarch64-scoped functions that call a mechanism primitive without settling the
/// per-CPU shadows themselves. Returned as `file::function` strings.
fn unsettled_primitive_callers(sources: &[(String, String)]) -> Vec<String> {
    let census = ttbr0_install_census(sources);
    let primitives: Vec<String> = census
        .iter()
        .filter(|install| {
            is_mechanism_primitive(
                &install.signature,
                &install.body,
                &install_operand(&install.body),
            )
        })
        .map(|install| match &install.owner {
            Some(owner) => format!("{owner}::{}", install.function),
            None => install.function.clone(),
        })
        .collect();

    let mut out = Vec::new();
    for (file, name, body) in aarch64_scoped_functions(sources) {
        if file.ends_with("arch_impl/aarch64/ttbr0.rs") {
            continue;
        }
        if !primitives
            .iter()
            .any(|primitive| calls_function(&body, primitive))
        {
            continue;
        }
        // The caller settles BOTH shadows itself, or hands the whole install to
        // the discipline that does. Publishing `saved_process_cr3` alone is not
        // enough: the corridor reads `next_cr3` first and installs it when it
        // holds a value other than 0, so a caller that leaves that word armed
        // has decided which root the next return to EL0 runs on just as surely
        // as a raw `msr`.
        if body.contains("adopt_process_ttbr0")
            || body.contains("quiesce_ttbr0_for_exit")
            || settles_both_shadows(&body)
        {
            continue;
        }
        // MMU bring-up: this install runs while TCR and MAIR are being
        // programmed, before there is per-CPU state to shadow, and it installs
        // the kernel root rather than a process root.
        if body.contains("msr tcr_el1") {
            continue;
        }
        out.push(format!("{file}::{name}"));
    }
    out
}

#[test]
fn every_ttbr0_install_settles_the_per_cpu_shadows() {
    let sources = rust_sources_below("kernel/src");
    let census = ttbr0_install_census(&sources);
    assert!(
        census.len() >= 5,
        "the TTBR0 install census reached only {} functions, so it is not covering the code it \
         claims to",
        census.len()
    );

    let mut unreconciled = Vec::new();
    for install in &census {
        if install.file == "kernel/src/arch_impl/aarch64/ttbr0.rs" {
            continue;
        }
        if settles_both_shadows(&install.body) {
            continue;
        }
        let operand = install_operand(&install.body);
        if is_mechanism_primitive(&install.signature, &install.body, &operand) {
            continue;
        }
        unreconciled.push(format!("{}::{}", install.file, install.function));
    }

    let escaped: Vec<&String> = unreconciled
        .iter()
        .filter(|entry| {
            !TIER_ONE_PROHIBITED
                .iter()
                .any(|tier_one| entry.starts_with(tier_one))
        })
        .collect();
    assert!(
        escaped.is_empty(),
        "these TTBR0 installs leave one or both per-CPU shadows naming another root: \
         {escaped:?}"
    );

    // Print, rather than pin, what the Tier-1 rule is holding back: these are
    // real members of the same defect class that this branch was not allowed to
    // touch. If the list empties because someone repaired them, the test still
    // passes; if it empties because the census stopped reaching them, the
    // coverage floor above is what notices.
    if !unreconciled.is_empty() {
        eprintln!(
            "TTBR0 installs still unreconciled behind the Tier-1 rule: {unreconciled:?}"
        );
    }
}

#[test]
fn the_shadow_census_rejects_an_unreconciled_install() {
    let invented = "fn install_a_process_root() {\n\
        let ttbr0_value = page_table.level_4_frame().start_address().as_u64();\n\
        unsafe {\n\
            core::arch::asm!(\"msr ttbr0_el1, {}\", in(reg) ttbr0_value);\n\
        }\n\
    }\n";
    let sources = vec![(
        "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
        invented.to_string(),
    )];
    let census = ttbr0_install_census(&sources);
    let site = census
        .iter()
        .find(|install| install.function == "install_a_process_root")
        .expect("the census must see a newly added install");
    assert!(
        !site.body.contains("set_saved_process_cr3"),
        "the perturbed site must be unreconciled"
    );
    assert!(
        !traces_to_a_parameter(&site.signature, &site.body, &install_operand(&site.body)),
        "a root read out of the process manager must not be classified as parameter-borne"
    );
}

#[test]
fn the_shadow_census_accepts_a_mechanism_primitive() {
    let signature = "    unsafe fn write_root(addr: u64) {";
    let body = "let aligned = addr & 0x0000_FFFF_FFFF_F000; \
                core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) aligned);";
    assert!(
        traces_to_a_parameter(signature, body, &install_operand(body)),
        "a primitive that installs what its caller handed it must not be asked to own the shadows"
    );
}

#[test]
fn the_tier_one_exemption_matches_the_project_rule() {
    let claude_md = repo_text("CLAUDE.md");
    let prohibited = claude_md
        .split("PROHIBITED CODE SECTIONS")
        .nth(1)
        .expect("CLAUDE.md must carry its prohibited-sections table");
    for path in TIER_ONE_PROHIBITED {
        assert!(
            prohibited.contains(path),
            "{path} is exempted as Tier-1, but CLAUDE.md does not list it as prohibited"
        );
    }
}

#[test]
fn the_ttbr0_discipline_publishes_both_shadows() {
    let ttbr0 = repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs");
    let adopt = function_body(&ttbr0, "adopt_process_ttbr0");
    assert!(
        adopt.contains("msr ttbr0_el1"),
        "the discipline helper must be the one that installs the register"
    );
    assert!(
        adopt.contains("set_saved_process_cr3(ttbr0_value)"),
        "adopting a process root must publish it as the root the return corridor restores"
    );
    assert!(
        adopt.contains("set_next_cr3(0)"),
        "adopting a process root must retire the pending switch the corridor would apply first"
    );
}

#[test]
fn init_reaches_userspace_on_the_root_the_shadows_name() {
    let main = repo_text("kernel/src/main_aarch64.rs");
    let launch = function_body(&main, "launch_init_from_elf");
    assert!(
        launch.contains("ttbr0::adopt_process_ttbr0(ttbr0_value)"),
        "init is the one thread that reaches EL0 without a scheduler dispatch, so its own \
         install has to reconcile the shadows"
    );
    assert!(
        !launch.contains("msr ttbr0_el1"),
        "a raw install here leaves the idle redirect kernel root armed in next_cr3"
    );
    let install = launch.find("adopt_process_ttbr0").unwrap();
    let eret = launch.rfind("return_to_userspace(entry_point").unwrap();
    assert!(
        install < eret,
        "the shadows must agree with the register before the ERET to EL0"
    );
}

#[test]
fn the_primitive_exemption_rejects_a_fetched_root() {
    // R7-002's hazard in one function: the operand traces to a parameter only
    // because that parameter was handed to a fetch. The value was fetched, not
    // given.
    let signature = "    unsafe fn install_for(flags: u64) {";
    let body = "let root = Aarch64PerCpu::process_root(flags); \
                core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root);";
    let operand = install_operand(body);
    assert!(
        traces_to_a_parameter(signature, body, &operand),
        "the loose predicate is what this test exists to tighten, so it must still accept this"
    );
    assert!(
        !is_mechanism_primitive(signature, body, &operand),
        "a site that FETCHES the root it installs is deciding policy, not performing a write"
    );
}

#[test]
fn the_primitive_exemption_rejects_a_shadow_touching_install() {
    let signature = "    unsafe fn write_root(addr: u64) {";
    let body = "let aligned = addr & 0x0000_FFFF_FFFF_F000; \
                core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) aligned); \
                Aarch64PerCpu::set_next_cr3(0);";
    assert!(
        !is_mechanism_primitive(signature, body, &install_operand(body)),
        "a function that touches one shadow is a policy site and owns both of them"
    );
}

#[test]
fn every_aarch64_caller_of_a_mechanism_primitive_settles_the_shadows() {
    let sources = rust_sources_below("kernel/src");
    let census = ttbr0_install_census(&sources);
    let primitives: Vec<&TtbrInstall> = census
        .iter()
        .filter(|install| {
            is_mechanism_primitive(
                &install.signature,
                &install.body,
                &install_operand(&install.body),
            )
        })
        .collect();
    assert!(
        !primitives.is_empty(),
        "the exemption this census polices reached no site at all, so it is checking nothing"
    );

    let unsettled = unsettled_primitive_callers(&sources);
    assert!(
        unsettled.is_empty(),
        "these aarch64 callers install a root through a mechanism primitive and leave the per-CPU \
         TTBR0 shadows naming another one: {unsettled:?}"
    );
}

#[test]
fn the_caller_census_catches_a_wrapper_that_skips_the_discipline() {
    // The exemption transfers the obligation to the caller. This is a caller
    // that does not discharge it: an aarch64 switch handing a process root
    // straight to the primitive.
    let primitive = "unsafe fn write_root(addr: u64) {\n\
        let aligned = addr & 0x0000_FFFF_FFFF_F000;\n\
        core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) aligned);\n\
    }\n";
    let skipping_caller = "#[cfg(target_arch = \"aarch64\")]\n\
        pub unsafe fn switch_to_root(page_table: &ProcessPageTable) {\n\
            let root = page_table.level_4_frame().start_address().as_u64();\n\
            Aarch64PageTableOps::write_root(root);\n\
        }\n";
    let sources = vec![
        (
            "kernel/src/arch_impl/aarch64/paging.rs".to_string(),
            primitive.to_string(),
        ),
        (
            "kernel/src/memory/process_memory.rs".to_string(),
            skipping_caller.to_string(),
        ),
    ];
    assert_eq!(
        unsettled_primitive_callers(&sources),
        vec!["kernel/src/memory/process_memory.rs::switch_to_root".to_string()],
        "a wrapper that routes a process root around the discipline has to be caught"
    );

    let settled = skipping_caller.replace(
        "Aarch64PageTableOps::write_root(root);",
        "crate::arch_impl::aarch64::ttbr0::adopt_process_ttbr0(root);",
    );
    let repaired = vec![
        (
            "kernel/src/arch_impl/aarch64/paging.rs".to_string(),
            primitive.to_string(),
        ),
        ("kernel/src/memory/process_memory.rs".to_string(), settled),
    ];
    assert!(
        unsettled_primitive_callers(&repaired).is_empty(),
        "routing the same switch through the discipline has to clear it"
    );
}

/// The first argument `body` passes to `call`, trimmed, once per call site, in
/// source order.
///
/// R3-N-004: reading only the FIRST occurrence is what this replaced. A body
/// that clears `next_cr3` and then arms it again a few lines later satisfied
/// the old reader, because the old reader stopped at the first call.
fn call_arguments(body: &str, call: &str) -> Vec<String> {
    let needle = format!("{call}(");
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = body[cursor..].find(&needle) {
        let at = cursor + offset;
        let open = at + needle.len();
        let is_longer_identifier = body[..at]
            .chars()
            .last()
            .map(|character| character.is_alphanumeric() || character == '_')
            .unwrap_or(false);
        let end = match body[open..].find(')') {
            Some(end) => end,
            None => break,
        };
        if !is_longer_identifier {
            out.push(body[open..open + end].trim().to_string());
        }
        cursor = open + end;
    }
    out
}

/// The LAST argument `body` passes to `call`, trimmed, if it calls it. What the
/// return corridor reads is whatever the final store to a shadow word left
/// there, so that is the write the predicates below score.
fn last_call_argument(body: &str, call: &str) -> Option<String> {
    call_arguments(body, call).pop()
}

/// Whether `body` leaves BOTH per-CPU TTBR0 shadow words describing the root it
/// just installed: it publishes a root into `saved_process_cr3`, and it retires
/// the pending switch by clearing `next_cr3`.
///
/// This is a shape rather than a list of blessed function names: any site that
/// keeps the corridor's two words in agreement with the register satisfies it,
/// and a site that settles only one of them does not. The asymmetry matters
/// because the corridor reads `next_cr3` FIRST and installs it when it holds a
/// value other than 0, so a stale `next_cr3` outranks a correct
/// `saved_process_cr3`.
///
/// R3-N-004: each accessor is scored on the LAST write the body makes, not the
/// first. What the corridor reads once the body has run is whatever the final
/// store left behind, so a body that clears `next_cr3` and then arms it again
/// with a root has not retired it -- and under the previous first-occurrence
/// reader that body passed.
fn settles_both_shadows(body: &str) -> bool {
    let publishes = last_call_argument(body, "set_saved_process_cr3")
        .is_some_and(|root| !root.is_empty() && root != "0");
    let retires =
        last_call_argument(body, "set_next_cr3").is_some_and(|pending| pending == "0");
    publishes && retires
}

/// Whether `body` leaves BOTH per-CPU TTBR0 shadow words holding a literal `0`.
///
/// This is the disposition a KERNEL-root install has to leave behind. There is
/// no process root for either word to name, so the corridor has to be told it
/// has no pending root to install and no saved root to fall back on.
/// `settles_both_shadows` is the
/// process-root disposition and deliberately rejects a `saved_process_cr3` of
/// 0; this is its counterpart, and the two are not interchangeable. Scored on
/// the LAST write to each word for the same reason.
fn zeroes_both_shadows(body: &str) -> bool {
    let cleared =
        last_call_argument(body, "set_saved_process_cr3").is_some_and(|root| root == "0");
    let retired =
        last_call_argument(body, "set_next_cr3").is_some_and(|pending| pending == "0");
    cleared && retired
}

/// The `asm!` invocation inside `body` that writes TTBR0_EL1, as the source
/// text between its outermost parentheses. A body that installs no root has no
/// such block, and the callers below treat that case as "not an installer".
fn ttbr0_install_asm_block(body: &str) -> Option<&str> {
    let install = body.find("msr ttbr0_el1")?;
    let macro_at = body[..install].rfind("asm!")? + "asm!".len();
    let open = macro_at + body[macro_at..].find('(')?;
    let mut depth = 0usize;
    for (offset, byte) in body.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&body[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the install block in `body` tells the compiler it touches no memory.
///
/// `nomem` is a licence to move memory accesses across the block. At a TTBR0
/// install that licence covers the per-CPU shadow stores that have to follow
/// the install and the page-table stores that have to precede it, so an
/// installer must not carry it. Read off the extracted block rather than the
/// whole function body, so a comment that names the option cannot decide the
/// answer.
fn install_block_is_nomem(body: &str) -> bool {
    ttbr0_install_asm_block(body).is_some_and(|block| block.contains("nomem"))
}

/// The six steps a TTBR0 install performs in this kernel, in order.
const INSTALL_SEQUENCE: [&str; 6] = [
    "dsb ishst",
    "msr ttbr0_el1",
    "isb",
    "tlbi vmalle1is",
    "dsb ish",
    "isb",
];

/// Whether `block` performs `INSTALL_SEQUENCE` in order.
fn performs_install_sequence(block: &str) -> bool {
    let mut cursor = 0usize;
    for step in INSTALL_SEQUENCE {
        match block[cursor..].find(step) {
            Some(offset) => cursor += offset + step.len(),
            None => return false,
        }
    }
    true
}

/// The functions in the discipline module that write TTBR0_EL1, as
/// (name, body). Discovered by shape, so a helper added to the module later is
/// covered without editing this test.
/// claim-lint:ok: the caller asserts equality against the module's own install
/// count, so the 2 helpers reached at this head are 2 of 2.
fn discipline_installers(source: &str) -> Vec<(String, String)> {
    declared_function_names(source)
        .into_iter()
        .filter_map(|name| {
            let body = function_body(source, &name).to_string();
            body.contains("msr ttbr0_el1").then_some((name, body))
        })
        .collect()
}

#[test]
fn the_discipline_installs_in_order_and_orders_the_shadow_stores() {
    let ttbr0 = repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs");
    let installers = discipline_installers(&ttbr0);

    // Coverage, not a name list: the install occurrences the walk reaches must
    // equal the occurrences the module holds, so a new helper cannot be added
    // past this test.
    // claim-lint:ok: the assertion below is that equality, 2 of 2 at this head.
    let reached: usize = installers
        .iter()
        .map(|(_, body)| body.matches("msr ttbr0_el1").count())
        .sum();
    let total = ttbr0.matches("msr ttbr0_el1").count();
    assert_eq!(
        reached, total,
        "the discipline module holds {total} TTBR0 installs but this check reached {reached}"
    );
    assert!(
        total >= 1,
        "the discipline module installs nothing, so this test is checking nothing"
    );

    for (name, body) in &installers {
        let block = ttbr0_install_asm_block(body)
            .unwrap_or_else(|| panic!("{name}: find the asm block that installs TTBR0"));
        assert!(
            performs_install_sequence(block),
            "{name}: the install must run {INSTALL_SEQUENCE:?} in order, so the root is visible \
             before the register takes it and no stale translation survives it"
        );
        assert!(
            !install_block_is_nomem(body),
            "{name}: the install block carries `nomem`, which tells the compiler it reads and \
             writes no memory -- a licence to move the per-CPU shadow stores and the caller's \
             page-table stores across the barriers"
        );
    }
}

#[test]
fn no_ttbr0_installer_claims_it_touches_no_memory() {
    let sources = rust_sources_below("kernel/src");
    let census = ttbr0_install_census(&sources);
    assert!(
        census.len() >= 5,
        "the TTBR0 install census reached only {} functions, so it is not covering the code it \
         claims to",
        census.len()
    );

    let mut nomem: Vec<String> = Vec::new();
    for install in &census {
        if install_block_is_nomem(&install.body) {
            nomem.push(format!("{}::{}", install.file, install.function));
        }
    }

    let escaped: Vec<&String> = nomem
        .iter()
        .filter(|entry| {
            !TIER_ONE_PROHIBITED
                .iter()
                .any(|tier_one| entry.starts_with(tier_one))
        })
        .collect();
    assert!(
        escaped.is_empty(),
        "these TTBR0 installs are declared `nomem`, so the compiler may move the surrounding \
         shadow and page-table stores across the barriers: {escaped:?}"
    );

    // Same disposition as the shadow census: print what the Tier-1 rule holds
    // back rather than pinning it, so a repair there cannot redden this test
    // and a coverage regression is still caught by the floor above.
    if !nomem.is_empty() {
        eprintln!("TTBR0 installs still `nomem` behind the Tier-1 rule: {nomem:?}");
    }
}

#[test]
fn the_nomem_check_reads_the_asm_block_and_not_the_prose() {
    let carrying = "unsafe {\n\
        core::arch::asm!(\"dsb ishst\", \"msr ttbr0_el1, {0}\", in(reg) root, \
        options(nomem, nostack));\n\
    }";
    assert!(
        install_block_is_nomem(carrying),
        "an install block that carries the option has to be caught"
    );

    let repaired = carrying.replace("options(nomem, nostack)", "options(nostack)");
    assert!(
        !install_block_is_nomem(&repaired),
        "dropping the option has to clear the check"
    );

    let commented = format!("// nomem would be wrong here\n{repaired}");
    assert!(
        !install_block_is_nomem(&commented),
        "prose naming the option must not decide the answer"
    );

    assert!(
        !performs_install_sequence("core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root)"),
        "a bare install with no barriers must not pass the sequence check"
    );
}

#[test]
fn the_dispatch_ttbr0_switch_settles_both_shadows() {
    // The scheduler's own install is the one a userspace thread takes on each
    // dispatch that changes address space, so it is pinned here rather than
    // only through the census.
    let context_switch = repo_text("kernel/src/arch_impl/aarch64/context_switch.rs");
    let switch = function_body(&context_switch, "switch_ttbr0_if_needed");
    let operand = install_operand(switch);
    assert!(
        switch.contains("msr ttbr0_el1"),
        "the dispatch switch must be the site that installs the register"
    );
    assert!(!operand.is_empty(), "find the value the dispatch switch installs");
    assert_eq!(
        last_call_argument(switch, "set_saved_process_cr3").as_deref(),
        Some(operand.as_str()),
        "the root the corridor restores must be the root this dispatch just installed"
    );
    assert!(
        settles_both_shadows(switch),
        "the dispatch switch must also retire the pending switch it consumed: a `next_cr3` left \
         armed is installed FIRST on the next return to EL0"
    );
    assert!(
        !install_block_is_nomem(switch),
        "the dispatch install must not tell the compiler it touches no memory"
    );
}

#[test]
fn settling_one_shadow_is_not_settling_the_shadows() {
    let both = "core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root); \
                Aarch64PerCpu::set_saved_process_cr3(root); \
                Aarch64PerCpu::set_next_cr3(0);";
    assert!(
        settles_both_shadows(both),
        "an install that settles both words is what the discipline looks like"
    );

    let saved_only = "core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root); \
                      Aarch64PerCpu::set_saved_process_cr3(root);";
    assert!(
        !settles_both_shadows(saved_only),
        "publishing `saved_process_cr3` alone leaves the word the corridor reads first armed"
    );

    let pending_only = "core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root); \
                        Aarch64PerCpu::set_next_cr3(0);";
    assert!(
        !settles_both_shadows(pending_only),
        "clearing `next_cr3` alone leaves the fallback root naming someone else"
    );

    let armed = "core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) root); \
                 Aarch64PerCpu::set_saved_process_cr3(root); \
                 Aarch64PerCpu::set_next_cr3(other_root);";
    assert!(
        !settles_both_shadows(armed),
        "leaving a non-zero pending switch behind is not retiring it"
    );
}

#[test]
fn the_caller_census_rejects_a_half_settled_caller() {
    // The same asymmetry one level out: a caller that hands a process root to a
    // mechanism primitive and publishes only `saved_process_cr3`.
    let primitive = "unsafe fn write_root(addr: u64) {\n\
        let aligned = addr & 0x0000_FFFF_FFFF_F000;\n\
        core::arch::asm!(\"msr ttbr0_el1, {0}\", in(reg) aligned);\n\
    }\n";
    let half = "#[cfg(target_arch = \"aarch64\")]\n\
        pub unsafe fn switch_to_root(page_table: &ProcessPageTable) {\n\
            let root = page_table.level_4_frame().start_address().as_u64();\n\
            Aarch64PageTableOps::write_root(root);\n\
            Aarch64PerCpu::set_saved_process_cr3(root);\n\
        }\n";
    let sources = vec![
        (
            "kernel/src/arch_impl/aarch64/paging.rs".to_string(),
            primitive.to_string(),
        ),
        (
            "kernel/src/memory/process_memory.rs".to_string(),
            half.to_string(),
        ),
    ];
    assert_eq!(
        unsettled_primitive_callers(&sources),
        vec!["kernel/src/memory/process_memory.rs::switch_to_root".to_string()],
        "a caller that publishes one shadow word and leaves the other armed has to be caught"
    );

    let settled = half.replace(
        "Aarch64PerCpu::set_saved_process_cr3(root);",
        "Aarch64PerCpu::set_saved_process_cr3(root);\n            \
         Aarch64PerCpu::set_next_cr3(0);",
    );
    let repaired = vec![
        (
            "kernel/src/arch_impl/aarch64/paging.rs".to_string(),
            primitive.to_string(),
        ),
        ("kernel/src/memory/process_memory.rs".to_string(), settled),
    ];
    assert!(
        unsettled_primitive_callers(&repaired).is_empty(),
        "settling both words has to clear the caller"
    );
}

// ---------------------------------------------------------------------------
// R3-N-003: the install sequence, applied kernel-wide
// ---------------------------------------------------------------------------

/// The censused installs the sequence requirement applies to, and those among
/// them whose asm block does not perform `INSTALL_SEQUENCE` in order, each as
/// `file::function`.
///
/// The requirement applies to each censused install that is NOT a mechanism
/// primitive. A primitive installs a value its caller chose and is exempt here
/// for the same reason it is exempt from owning the shadows: the caller decided
/// what the register would hold and therefore owns what has to be invalidated
/// around it. That exemption is a real narrowing -- 2 of 2 primitives at this
/// head (`paging.rs::write_root` and `memory/arch_stub.rs::Cr3::write`) run
/// `dsb ishst` / `msr` / `dsb ish` / `isb`, with no `isb` after the `msr` and
/// no `tlbi vmalle1is`, so neither would pass this check if it were applied to
/// them. Saying so is the point of writing it down; this test makes no claim
/// that their callers compensate.
fn install_sequence_census(sources: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    let census = ttbr0_install_census(sources);
    let mut checked = Vec::new();
    let mut out_of_order = Vec::new();
    for install in &census {
        let operand = install_operand(&install.body);
        if is_mechanism_primitive(&install.signature, &install.body, &operand) {
            continue;
        }
        let site = format!("{}::{}", install.file, install.function);
        let ordered = ttbr0_install_asm_block(&install.body).is_some_and(performs_install_sequence);
        checked.push(site.clone());
        if !ordered {
            out_of_order.push(site);
        }
    }
    (checked, out_of_order)
}

#[test]
fn every_non_primitive_ttbr0_install_performs_the_install_sequence() {
    let sources = rust_sources_below("kernel/src");
    let (checked, out_of_order) = install_sequence_census(&sources);

    // Coverage, expressed as census shape rather than a name list: the check
    // has to reach several installs, and it has to reach them in more than one
    // file. `the_discipline_installs_in_order_and_orders_the_shadow_stores`
    // already covers the discipline module on its own, so a version of this
    // that collapsed back to that one file would say no more than that check
    // already says.
    assert!(
        checked.len() >= 4,
        "the sequence check reached only {} non-primitive installs, so it is not covering the \
         code it claims to: {checked:?}",
        checked.len()
    );
    let files: BTreeSet<&str> = checked
        .iter()
        .filter_map(|site| site.split("::").next())
        .collect();
    assert!(
        files.len() >= 2,
        "the sequence check reached installs in only one file, so it says nothing the \
         discipline-module check did not already say: {checked:?}"
    );

    let escaped: Vec<&String> = out_of_order
        .iter()
        .filter(|entry| {
            !TIER_ONE_PROHIBITED
                .iter()
                .any(|tier_one| entry.starts_with(tier_one))
        })
        .collect();
    assert!(
        escaped.is_empty(),
        "these TTBR0 installs do not run {INSTALL_SEQUENCE:?} in order, so a stale translation \
         can survive the install or the root can be taken before it is visible: {escaped:?}"
    );

    // Same disposition as the other kernel-wide censuses: print what the Tier-1
    // rule holds back rather than pinning it.
    if !out_of_order.is_empty() {
        eprintln!("TTBR0 installs still out of sequence behind the Tier-1 rule: {out_of_order:?}");
    }
}

#[test]
fn the_sequence_census_catches_an_install_outside_the_discipline_module() {
    let truncated = "fn install_a_process_root() {\n\
        let root = page_table.level_4_frame().start_address().as_u64();\n\
        unsafe {\n\
            core::arch::asm!(\"dsb ishst\", \"msr ttbr0_el1, {0}\", \"isb\", in(reg) root);\n\
            Aarch64PerCpu::set_saved_process_cr3(root);\n\
            Aarch64PerCpu::set_next_cr3(0);\n\
        }\n\
    }\n";
    let sources = vec![(
        "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
        truncated.to_string(),
    )];
    assert_eq!(
        install_sequence_census(&sources).1,
        vec!["kernel/src/arch_impl/aarch64/invented.rs::install_a_process_root".to_string()],
        "an install that stops after the `isb` leaves stale user translations behind and has to \
         be caught outside the discipline module too"
    );

    let repaired = truncated.replace(
        "\"isb\", in(reg) root",
        "\"isb\", \"tlbi vmalle1is\", \"dsb ish\", \"isb\", in(reg) root",
    );
    let sources = vec![(
        "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
        repaired,
    )];
    assert!(
        install_sequence_census(&sources).1.is_empty(),
        "running the whole sequence has to clear it"
    );
}

// ---------------------------------------------------------------------------
// R3-N-005: the callers of the kernel-root install
// ---------------------------------------------------------------------------

/// The text inside the `without_interrupts( ... )` call in `body` that
/// lexically encloses the first call to `call`, if there is one.
fn masked_window_around<'a>(body: &'a str, call: &str) -> Option<&'a str> {
    const WINDOW: &str = "without_interrupts(";
    let needle = format!("{call}(");
    let target = body.find(&needle)?;
    let mut cursor = 0usize;
    while let Some(offset) = body[cursor..].find(WINDOW) {
        let at = cursor + offset;
        let open = at + WINDOW.len() - 1;
        let mut depth = 0usize;
        let mut close = None;
        for (index, byte) in body.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close?;
        if open < target && target < close {
            return Some(&body[open + 1..close]);
        }
        cursor = at + WINDOW.len();
    }
    None
}

/// aarch64-scoped functions that install the KERNEL root and leave the per-CPU
/// TTBR0 shadows naming something else. Returned as `file::function` strings.
///
/// `switch_ttbr0_to_kernel` settles neither shadow, by design: it is the
/// mechanism, and the kernel root is not a value either corridor arm may
/// install on a return to EL0. That makes the obligation the caller's, and
/// before round 3 no check covered it -- the primitive-caller census next door
/// skips the discipline module outright and does not reach this helper. A
/// caller discharges the obligation one of two ways:
///
///   * it leaves both words settled itself -- zeroed, because no process root
///     is live on this CPU any more (`quiesce_ttbr0_for_exit`,
///     `sys_exit_aarch64`), or naming a process root it went on to install; or
///   * the kernel-root install and the reinstall that ends it are one
///     interrupt-masked window, and BOTH ways out of that window go through a
///     helper that settles both words. `sys_exec_aarch64` is that shape:
///     `adopt_process_ttbr0` on the success arm and
///     `restore_ttbr0_after_failed_exec` on the failure arm, inside the
///     `without_interrupts` closure that also holds the kernel-root install.
///     That window is pinned by two other suites, not by this one:
///     `tests/context_restore_structure.rs`'s
///     `validate_aarch64_failed_exec_ttbr0_rollback` requires the capture, the
///     kernel-root transition and the `exec_process_with_argv` call to appear
///     exactly once in that order and the `Err` arm to roll back before any
///     return, and `tests/exec_lock_order_structure.rs`'s
///     `validate_sys_exec_releases_process_manager` requires exactly one
///     `adopt_process_ttbr0(` after `commit.apply()` and no raw `msr
///     ttbr0_el1` anywhere in the function.
fn unsettled_kernel_root_callers(sources: &[(String, String)]) -> Vec<String> {
    const KERNEL_ROOT_INSTALL: &str = "switch_ttbr0_to_kernel";
    let mut out = Vec::new();
    for (file, name, body) in aarch64_scoped_functions(sources) {
        if name == KERNEL_ROOT_INSTALL || !calls_function(&body, KERNEL_ROOT_INSTALL) {
            continue;
        }
        if zeroes_both_shadows(&body) || settles_both_shadows(&body) {
            continue;
        }
        if masked_window_around(&body, KERNEL_ROOT_INSTALL).is_some_and(|window| {
            window.contains("adopt_process_ttbr0(")
                && window.contains("restore_ttbr0_after_failed_exec(")
        }) {
            continue;
        }
        out.push(format!("{file}::{name}"));
    }
    out
}

#[test]
fn every_caller_of_the_kernel_root_install_settles_the_shadows() {
    let sources = rust_sources_below("kernel/src");

    let callers: Vec<String> = aarch64_scoped_functions(&sources)
        .into_iter()
        .filter(|(_, name, body)| {
            name != "switch_ttbr0_to_kernel" && calls_function(body, "switch_ttbr0_to_kernel")
        })
        .map(|(file, name, _)| format!("{file}::{name}"))
        .collect();
    assert!(
        callers.len() >= 2,
        "the kernel-root install census reached {} callers, so it is checking nothing: \
         {callers:?}",
        callers.len()
    );

    let unsettled = unsettled_kernel_root_callers(&sources);
    assert!(
        unsettled.is_empty(),
        "these aarch64 callers install the kernel root and leave the per-CPU TTBR0 shadows \
         naming another one, so the next return to EL0 may reinstall a root this CPU has just \
         left: {unsettled:?}"
    );
}

#[test]
fn the_kernel_root_caller_census_catches_a_caller_that_leaves_the_shadows_armed() {
    let discipline = "pub fn switch_ttbr0_to_kernel() {\n\
        let ttbr0 = kernel_ttbr0();\n\
        unsafe {\n\
            core::arch::asm!(\"msr ttbr0_el1, {ttbr0}\", ttbr0 = in(reg) ttbr0);\n\
        }\n\
    }\n";
    let bare = "#[cfg(target_arch = \"aarch64\")]\n\
        pub fn retire_the_address_space() {\n\
            switch_ttbr0_to_kernel();\n\
        }\n";
    let with = |caller: &str| {
        vec![
            (
                "kernel/src/arch_impl/aarch64/ttbr0.rs".to_string(),
                discipline.to_string(),
            ),
            ("kernel/src/task/retire.rs".to_string(), caller.to_string()),
        ]
    };
    let site = "kernel/src/task/retire.rs::retire_the_address_space".to_string();

    assert_eq!(
        unsettled_kernel_root_callers(&with(bare)),
        vec![site.clone()],
        "a caller that installs the kernel root and touches neither shadow leaves the corridor \
         free to reinstall the root it just left"
    );

    let zeroed = bare.replace(
        "switch_ttbr0_to_kernel();",
        "switch_ttbr0_to_kernel();\n            \
         Aarch64PerCpu::set_saved_process_cr3(0);\n            \
         Aarch64PerCpu::set_next_cr3(0);",
    );
    assert!(
        unsettled_kernel_root_callers(&with(&zeroed)).is_empty(),
        "zeroing both words is one of the two ways to discharge the obligation"
    );

    let half = bare.replace(
        "switch_ttbr0_to_kernel();",
        "switch_ttbr0_to_kernel();\n            Aarch64PerCpu::set_saved_process_cr3(0);",
    );
    assert_eq!(
        unsettled_kernel_root_callers(&with(&half)),
        vec![site.clone()],
        "zeroing one word and leaving the other armed is not settling the shadows"
    );

    let masked = bare.replace(
        "switch_ttbr0_to_kernel();",
        "let result = without_interrupts(|| {\n                \
         switch_ttbr0_to_kernel();\n                \
         let Ok(root) = replace_address_space() else {\n                    \
         restore_ttbr0_after_failed_exec(previous);\n                    \
         return -12;\n                };\n                \
         adopt_process_ttbr0(root);\n                \
         0\n            });",
    );
    assert!(
        unsettled_kernel_root_callers(&with(&masked)).is_empty(),
        "the exec shape -- one masked window whose every exit reinstalls through a helper that \
         settles both words -- is the other way to discharge it"
    );

    let one_armed_window = masked.replace(
        "restore_ttbr0_after_failed_exec(previous);\n                    ",
        "",
    );
    assert_eq!(
        unsettled_kernel_root_callers(&with(&one_armed_window)),
        vec![site],
        "a masked window whose failure arm returns with the kernel root still installed is not \
         exempt"
    );
}
