//! Structural regressions for the aarch64 TTBR0 shadow-reconciliation
//! discipline (#786).
//!
//! The per-CPU words the syscall return corridor reads decide which page-table
//! root the next return to EL0 runs on. These checks pin the shape that keeps
//! every process-root install and the shadows describing it in agreement. They
//! are intentionally about behavior-bearing call shapes rather than line
//! numbers.
//! claim-lint:ok: 9 of the 10 censused process-root installs are routed
//! through the discipline at this head; the 10th is the Tier-1 site
//! `kernel/src/syscall/time.rs::ensure_current_address_space`, which
//! `every_ttbr0_install_settles_the_per_cpu_shadows` prints on every run.


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
/// claim-lint:ok: 7 censused functions at this head, enumerated in
/// docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
///
/// The walk is name-driven, so what it carries is a coverage FLOOR: the install
/// occurrences inside censused bodies must be at least as many as the file
/// holds. Nested functions can be double-counted, so the floor does not by
/// itself exclude one hidden site paired with one double-count; what it does
/// catch is a file whose installs the name walk missed outright.
/// claim-lint:ok: 7 of 7 censused at this head, enumerated in
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
/// helper. 7 of the 7 censused TTBR0 install sites follow one of the 2
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
        // The caller settles the shadows itself, or hands the whole install to
        // the discipline that does.
        if body.contains("adopt_process_ttbr0")
            || body.contains("quiesce_ttbr0_for_exit")
            || body.contains("set_saved_process_cr3")
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
        if install.body.contains("set_saved_process_cr3") {
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
        "these TTBR0 installs leave the per-CPU shadows naming another root: {escaped:?}"
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
