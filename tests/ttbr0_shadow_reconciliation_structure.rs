//! Structural regressions for the aarch64 TTBR0 shadow-reconciliation
//! discipline (#786).
//!
//! The per-CPU words the syscall return corridor reads decide which page-table
//! root the next return to EL0 runs on. These checks pin the shape that keeps
//! each process-root install and the shadows describing it in agreement. They
//! are intentionally about behavior-bearing call shapes rather than line
//! numbers.
//!
//! Two different things are counted here and they are not the same number, so
//! both are stated rather than left to be inferred:
//!
//!   * 10 process-root install DECISION sites existed on `main` before this
//!     work -- the places that chose a root and put it in TTBR0_EL1. Slice 1
//!     routed 9 of the 10 through `ttbr0::adopt_process_ttbr0`; slice 1b
//!     routed the 10th,
//!     `kernel/src/syscall/time.rs::ensure_current_address_space`, under the
//!     Tier-1 approval slice 1 did not have (operator ruling R156). 10 of 10
//!     are routed at this head.
//!   * 6 FUNCTIONS still write TTBR0_EL1 with a raw `msr` at this head, and
//!     those 6 are what the census below walks: 2 discipline-module helpers,
//!     2 that reconcile both shadows inline, and 2 mechanism primitives that
//!     install what a caller decided.
//!
//! The 10 routed sites are absent from the 6 precisely because they no longer
//! write the register themselves. Both accountings are enumerated in
//! docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md
//! claim-lint:ok: the 6 censused functions and their dispositions are printed
//! by `every_ttbr0_install_settles_the_per_cpu_shadows -- --nocapture`, which
//! is recorded verbatim under
//! docs/planning/green-program/aarch64-testing/serials/slice1b/


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
//   * everything else, which must be empty. There is no file-scoped exemption:
//     slice 1b routed the last unreconciled install, which lived in a Tier-1
//     file, so this census requires zero unreconciled installs kernel-wide.
// claim-lint:ok: the censused functions and their dispositions are printed by
// `every_ttbr0_install_settles_the_per_cpu_shadows -- --nocapture` and recorded
// in docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md

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
    let mut disposition = Vec::new();
    for install in &census {
        let site = format!("{}::{}", install.file, install.function);
        if install.file == "kernel/src/arch_impl/aarch64/ttbr0.rs" {
            disposition.push(format!("{site} (the discipline)"));
            continue;
        }
        if settles_both_shadows(&install.body) {
            disposition.push(format!("{site} (reconciles inline)"));
            continue;
        }
        let operand = install_operand(&install.body);
        if is_mechanism_primitive(&install.signature, &install.body, &operand) {
            disposition.push(format!("{site} (parameter-borne)"));
            continue;
        }
        disposition.push(format!("{site} (unreconciled)"));
        unreconciled.push(site);
    }

    // The census, disclosed rather than summarised: a `--nocapture` run prints
    // every function the walk reached and how each was classified, so the
    // counts in the slice documents are reproducible from the tree instead of
    // being asserted about it. This is disclosure, not exemption -- the
    // assertion below is on the whole list.
    // claim-lint:ok: 6 of 6 censused functions are printed by this run at the
    // slice-1b head; recorded in
    // docs/planning/green-program/aarch64-testing/serials/slice1b/anti-vacuity/04-post-fix-green.txt
    eprintln!(
        "TTBR0 install census ({} functions): {disposition:#?}",
        disposition.len()
    );

    // No file-scoped exemption. Slice 1b routed the last raw process-root
    // install, `kernel/src/syscall/time.rs::ensure_current_address_space`,
    // through the discipline under operator ruling R156, so this census
    // requires zero unreconciled installs kernel-wide rather than printing a
    // list it declines to pin. Mechanism primitives keep the disposition
    // documented on `ttbr0_install_census`: they install a value their caller
    // chose, so the caller owns the shadows.
    // claim-lint:ok: 0 of 6 censused installs are unreconciled at this head;
    // docs/planning/green-program/aarch64-testing/serials/slice1b/anti-vacuity/04-post-fix-green.txt
    assert!(
        unreconciled.is_empty(),
        "these TTBR0 installs leave one or both per-CPU shadows naming another root: \
         {unreconciled:?}"
    );
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

    // Same disposition as the shadow census, and no file-scoped exemption
    // beside it: every censused install kernel-wide must be free of `nomem`.
    // claim-lint:ok: 0 of 6 censused installs carry `nomem` at this head;
    // docs/planning/green-program/aarch64-testing/serials/slice1b/anti-vacuity/04-post-fix-green.txt
    assert!(
        nomem.is_empty(),
        "these TTBR0 installs are declared `nomem`, so the compiler may move the surrounding \
         shadow and page-table stores across the barriers: {nomem:?}"
    );
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

    // Same disposition as the other kernel-wide censuses, and no file-scoped
    // exemption beside it: every non-primitive censused install must run the
    // sequence in order.
    // claim-lint:ok: 0 of 4 non-primitive censused installs are out of order at
    // this head;
    // docs/planning/green-program/aarch64-testing/serials/slice1b/anti-vacuity/04-post-fix-green.txt
    assert!(
        out_of_order.is_empty(),
        "these TTBR0 installs do not run {INSTALL_SEQUENCE:?} in order, so a stale translation \
         can survive the install or the root can be taken before it is visible: {out_of_order:?}"
    );
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

// ---------------------------------------------------------------------------
// The ASID the corridor carries back to EL0 (R157 / S1B-01)
// ---------------------------------------------------------------------------
//
// A process root is not just a physical address. The dispatch path tags the
// root it publishes with ASID 1, and the `.Lrestore_saved_ttbr` arm of
// `syscall_entry.S` installs the shadow word VERBATIM, ASID bits included. So a
// site that publishes an untagged root does not merely disagree with the
// dispatch path in a cosmetic field -- it decides that the next return to EL0
// runs on ASID 0, which is the ASID the boot identity map's TLB entries carry.
// claim-lint:ok: 1 of 1 dispatch tag (`set_next_ttbr0_for_thread`) and 1 of 1
// corridor arm (`.Lrestore_saved_ttbr`) are the two sites this paragraph is
// about; both are cited by path in
// docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md
// The checks below pin the tag as a property of the discipline: one constant,
// equal to the dispatch path's own tag, applied to the value that is both
// installed and published.

/// The `<mantissa> << <shift>` ASID tag in `expression`, parsed rather than
/// string-matched so a type suffix or a spacing difference cannot make two
/// identical tags compare unequal, and so a changed ASID cannot pass as one.
fn asid_tag(expression: &str) -> Option<(u64, u32)> {
    let (left, right) = expression.split_once("<<")?;
    let mantissa: String = left
        .trim_end()
        .chars()
        .rev()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let mantissa = mantissa
        .trim_end_matches("u64")
        .trim_end_matches("u32")
        .trim_end_matches("usize");
    let shift: String = right
        .trim_start()
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    Some((mantissa.parse().ok()?, shift.parse().ok()?))
}

#[test]
fn the_discipline_publishes_the_dispatch_asid() {
    let ttbr0 = repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs");
    let context_switch = repo_text("kernel/src/arch_impl/aarch64/context_switch.rs");

    // The dispatch path must REACH the tag through the normaliser, not spell
    // it. R157/ASID-05: this check used to parse a `1u64 << 48` out of
    // `set_next_ttbr0_for_thread` and compare it to the constant, which two
    // spellings of the same number both satisfy -- so the or-only form, the
    // one `process_root_ttbr0`'s doc comment says is refused, passed it. The
    // published value is read off the publish itself, so a renamed binding
    // does not silently drop the check.
    let dispatch_body = function_body(&context_switch, "set_next_ttbr0_for_thread");
    let published = last_call_argument(dispatch_body, "set_next_cr3")
        .expect("the dispatch path must publish a root into next_cr3");
    let dispatch_rhs = bound_expression(dispatch_body, &published).unwrap_or_else(|| {
        panic!("the published value {published:?} must be bound in the dispatch body")
    });
    assert!(
        dispatch_rhs.contains("process_root_ttbr0("),
        "the dispatch path publishes {published:?} without routing it through the \
         discipline's normalisation, so whatever ASID the operand already carried \
         survives into the word the corridor installs verbatim: {dispatch_rhs:?}"
    );

    let discipline_line = ttbr0
        .lines()
        .find(|line| line.contains("const USER_ASID_TTBR0"))
        .expect("the discipline must name the userspace ASID as a constant");
    asid_tag(discipline_line)
        .unwrap_or_else(|| panic!("parse the discipline ASID tag from {discipline_line:?}"));

    // The normalisation REPLACES the field. An or-only tag leaves a foreign
    // ASID a caller happened to hand over in place, which is the shape this
    // check exists to reject.
    let normalise = function_body(&ttbr0, "process_root_ttbr0");
    assert!(
        normalise.contains("& TTBR0_ROOT_MASK"),
        "normalising a process root must mask the ASID field before setting it, or a caller's own ASID bits survive"
    );
    assert!(
        normalise.contains("| USER_ASID_TTBR0"),
        "normalising a process root must set the userspace ASID"
    );

    // The value the register takes and the value the corridor is handed are the
    // same normalised binding -- not two expressions that happen to agree today.
    let adopt = function_body(&ttbr0, "adopt_process_ttbr0");
    let operand = install_operand(adopt);
    assert!(
        !operand.is_empty(),
        "the discipline must hand a named value to the install block"
    );
    let normalised = let_binding_rhs(adopt, &operand).unwrap_or_else(|| {
        panic!("the installed value {operand:?} must be bound in the discipline body")
    });
    assert!(
        normalised.contains("process_root_ttbr0("),
        "the discipline installs {operand:?} without normalising the ASID, so an untagged root reaches both the register and the corridor: {normalised:?}"
    );
    assert_eq!(
        last_call_argument(adopt, "set_saved_process_cr3").as_deref(),
        Some(operand.as_str()),
        "the corridor must be handed the same normalised value the register took"
    );
}

#[test]
fn the_asid_check_rejects_a_disagreeing_tag() {
    // Anti-vacuity for the comparison above: the parse is what carries it, so
    // it has to see a difference in the ASID where there is one, and no
    // difference where only the spelling differs.
    // claim-lint:ok: 5 of 5 cases below are the parse's whole contract -- 1
    // spelling-equivalence, 2 disagreements, 1 embedded tag, 1 non-tag.
    assert_eq!(asid_tag("1u64 << 48"), asid_tag("1 << 48"));
    assert_ne!(asid_tag("2u64 << 48"), asid_tag("1 << 48"));
    assert_ne!(asid_tag("1u64 << 49"), asid_tag("1 << 48"));
    assert_eq!(asid_tag("ttbr0 | (1u64 << 48)"), Some((1, 48)));
    assert_eq!(asid_tag("no shift here"), None);
}

// ---------------------------------------------------------------------------
// The blocking-resume restore (R157 / S1B-06)
// ---------------------------------------------------------------------------
//
// A function that resolves the CURRENT thread's own process root and installs
// it is not choosing an address space; it is recovering the one it already had
// after blocking. `ttbr0::restore_process_ttbr0` is that disposition: it
// publishes both corridor words unconditionally and skips the register write,
// and with it the inner-shareable broadcast invalidation, when TTBR0 already
// holds exactly that root under that ASID. Calling the adopt path from one of
// these sites instead issues a broadcast TLBI on a blocking return whose root
// has not changed. The census is by shape, so a sixth copy of the helper cannot
// be added past it.
// claim-lint:ok: 5 of 5 copies of the helper are the census this check prints;
// the run is recorded in
// docs/planning/green-program/aarch64-testing/serials/slice1b/r157/anti-vacuity/00-branch-green.txt

/// aarch64-scoped functions that resolve the current thread's own process root
/// and install it. Returns (the sites reached, the sites not using the guarded
/// restore), each as `file::function`.
///
/// Three conjuncts, and the third is what narrows this to the RESUME family
/// rather than any function that installs a root near a thread lookup:
///
///   * it looks the row up by the CURRENT thread -- `find_process_by_thread(`
///     matched with a plain `contains` rather than `calls_function`, which
///     rejects method calls, because on this shape the lookup is a method on
///     the process-manager guard. The `_mut` variant is a different shape (the
///     signal-delivery sites, which mutate the row they find) and does not
///     match this needle;
///   * it installs a process root through the discipline;
///   * and the value it installs is derived from THAT ROW'S page table. This is
///     the conjunct `sys_exec_aarch64` fails, correctly: it reaches the first
///     two, but the root it installs comes from its exec receipt
///     (`commit.new_page_table_root()`) -- a new address space it chose rather
///     than the one its own thread already owns -- so its install is an adopt
///     and must stay one.
/// claim-lint:ok: 5 of 5 sites satisfy all three conjuncts at this head and 1
/// (`sys_exec_aarch64`) satisfies the first two and is asserted excluded; the
/// census is printed by the test below and recorded in
/// docs/planning/green-program/aarch64-testing/serials/slice1b/r157/anti-vacuity/00-branch-green.txt
fn blocking_resume_restore_census(sources: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    const INSTALLS: [&str; 2] = ["restore_process_ttbr0", "adopt_process_ttbr0"];

    let mut reached = Vec::new();
    let mut unguarded = Vec::new();
    for (file, name, body) in aarch64_scoped_functions(sources) {
        if file.ends_with("arch_impl/aarch64/ttbr0.rs") {
            continue;
        }
        if !calls_function(&body, "current_thread_id") || !body.contains("find_process_by_thread(")
        {
            continue;
        }
        let guarded = calls_function(&body, "restore_process_ttbr0");
        let adopts = calls_function(&body, "adopt_process_ttbr0");
        if !guarded && !adopts {
            continue;
        }
        let installs_the_found_row = INSTALLS.iter().any(|call| {
            calls_function(&body, call)
                && last_call_argument(&body, call)
                    .and_then(|argument| let_binding_rhs(&body, &argument))
                    .is_some_and(|derivation| derivation.contains("level_4_frame()"))
        });
        if !installs_the_found_row {
            continue;
        }
        let site = format!("{file}::{name}");
        reached.push(site.clone());
        if adopts || !guarded {
            unguarded.push(site);
        }
    }
    (reached, unguarded)
}

#[test]
fn every_blocking_resume_restore_uses_the_guarded_helper() {
    let sources = rust_sources_below("kernel/src");
    let (reached, unguarded) = blocking_resume_restore_census(&sources);

    // Disclosure, not exemption: a `--nocapture` run prints the family, so the
    // count in the slice document is read out of the tree.
    eprintln!(
        "blocking-resume restore census ({} functions): {reached:#?}",
        reached.len()
    );

    // Coverage floor as census shape, not a name list. The family is the
    // per-syscall copies of one helper; a walk that stopped reaching them would
    // leave this check passing on an empty list, which is what the floor
    // rejects.
    // claim-lint:ok: 5 of 5 copies are reached at this head, in 5 of 5 distinct
    // files, and the mutation that reddens this check is run 03 in
    // docs/planning/green-program/aarch64-testing/serials/slice1b/r157/anti-vacuity/
    assert!(
        reached.len() >= 5,
        "the blocking-resume census reached only {} sites, so it is not covering the code it claims to: {reached:?}",
        reached.len()
    );
    let files: BTreeSet<&str> = reached
        .iter()
        .filter_map(|site| site.split("::").next())
        .collect();
    assert!(
        files.len() >= 5,
        "the blocking-resume census reached {} files, so it is not seeing the whole family: {reached:?}",
        files.len()
    );

    // `sys_exec_aarch64` reaches two of the three conjuncts and must not be a
    // member: it installs a root it chose, so its unconditional install and its
    // invalidation are both required. Pinned here so a future loosening of the
    // census shows up as a failure rather than as a silently widened family.
    assert!(
        !reached
            .iter()
            .any(|site| site.ends_with("::sys_exec_aarch64")),
        "exec installs a NEW address space, so it is not a blocking-resume restore: {reached:?}"
    );

    assert!(
        unguarded.is_empty(),
        "these sites re-install the root their own blocked thread already owns through the adopt path, so every blocking return issues a broadcast TLB invalidation whether or not the register moved: {unguarded:?}"
    );
}

#[test]
fn the_blocking_resume_census_catches_an_unguarded_restore() {
    // The same shape, installing through the adopt path: the census must name
    // it. Without this the check above would pass on an empty census.
    let invented = "#[cfg(target_arch = \"aarch64\")]\n\
        fn ensure_current_address_space() {\n\
            let thread_id = crate::task::scheduler::current_thread_id();\n\
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {\n\
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();\n\
                crate::arch_impl::aarch64::ttbr0::adopt_process_ttbr0(ttbr0_value);\n\
            }\n\
        }\n";
    let sources = vec![(
        "kernel/src/syscall/invented.rs".to_string(),
        invented.to_string(),
    )];
    let (reached, unguarded) = blocking_resume_restore_census(&sources);
    assert_eq!(
        reached,
        vec!["kernel/src/syscall/invented.rs::ensure_current_address_space".to_string()],
        "the census must see a newly added blocking-resume restore"
    );
    assert_eq!(
        unguarded, reached,
        "a blocking-resume restore that installs through the adopt path must be named"
    );

    // And the guarded spelling of the same function must not be named, so the
    // census is not simply flagging everything it reaches.
    let guarded = invented.replace("adopt_process_ttbr0", "restore_process_ttbr0");
    let sources = vec![("kernel/src/syscall/invented.rs".to_string(), guarded)];
    let (reached, unguarded) = blocking_resume_restore_census(&sources);
    assert_eq!(reached.len(), 1);
    assert!(
        unguarded.is_empty(),
        "the guarded spelling must pass: {unguarded:?}"
    );
}

// ---------------------------------------------------------------------------
// The VALUE a shadow publish carries (#786 follow-on)
// ---------------------------------------------------------------------------
//
// Everything above this line reads SHAPE: which function installs, whether it
// settles both words, whether the value it installs traces to a parameter. The
// defect that shipped on `main` for five hours passed every one of those
// checks. `adopt_process_ttbr0` settled both words, in order, from a named
// binding; what was wrong was the VALUE in the binding -- the caller's
// ASID-untagged root -- and the `.Lrestore_saved_ttbr` arm of
// `syscall_entry.S` installs that word verbatim, so a `nanosleep` or EINTR
// return went to EL0 on ASID 0 while a dispatch return went on ASID 1.
// claim-lint:ok: 25 of 27 tests in this file still pass with that value
// defect restored, and the 2 that redden are named in
// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/01-structural-anti-vacuity-raw-adopt.txt
//
// The brief for this round cites coordinator ruling R19 for why the shape
// ratchet could not see it. That ruling's text is not recorded in this
// repository, in issue #786 or in PR #800, so it is not quoted here; what IS
// in the tree is the shape of the miss, and this census is what pins it:
// `the_discipline_publishes_the_dispatch_asid` above checks ONE named
// function, so a second publish site added tomorrow with an untagged operand
// fails no check in this file.
// claim-lint:ok: 1 of 1 function that check reads is `adopt_process_ttbr0`,
// and the census below reaches 17 publishes across 5 functions --
// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/05-suite-green-with-census.txt
//
// This census is the value ratchet. It walks every call to the two per-CPU
// shadow accessors in aarch64-scoped code under `kernel/src` and requires
// every operand to have an ACCOUNTED provenance: a literal 0, a value
// normalised through `process_root_ttbr0`, a value carrying the dispatch tag,
// this CPU's kernel root, or a value read back out of the register or out of a
// shadow word. A raw `level_4_frame().start_address()` reaching a shadow store
// is exactly what has no provenance here.
// claim-lint:ok: 17 of 17 publishes and their dispositions are printed by
// `every_shadow_publish_has_an_accounted_asid -- --nocapture` in
// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/05-suite-green-with-census.txt
//
// Two disclosed narrowings, both covered by the RUNTIME census rather than by
// this one:
//
//   * the aarch64 scope is `aarch64_scoped_functions`' scope, documented on
//     that function: shared code with no cfg at all is outside it, and so is
//     `kernel/src/per_cpu_aarch64.rs`, whose module declaration carries the
//     cfg its path does not. The runtime counter sits at the per-CPU write
//     itself, so a publish from any of those still reaches it.
//   * a value read back out of the hardware register carries whatever ASID was
//     installed, and no source shape can say what that was. The runtime
//     counter is what measures it.

/// The two per-CPU accessors that WRITE a corridor shadow word. The census is
/// keyed on these names, not on a list of files: a new publish site is reached
/// because it has to call one of them.
const SHADOW_WRITERS: [&str; 2] = ["set_saved_process_cr3", "set_next_cr3"];

/// How a published value got its ASID field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Provenance {
    /// A literal 0. The corridor arm for that word is disarmed, and 0 is the
    /// one value `syscall_entry.S` tests for before installing.
    Cleared,
    /// Normalised through `process_root_ttbr0` in this function.
    Normalised,
    /// The ASID tag is spelled here, by hand, rather than reached through
    /// `process_root_ttbr0`. This is NOT accounted (R157/ASID-05): the
    /// discipline's normalisation REPLACES bits [63:48], and its own doc
    /// comment refuses the or-only form because that preserves a foreign ASID
    /// the operand already carried. A ratchet that accepts both spellings
    /// cannot tell "replaced" from "or-ed", which is exactly the distinction
    /// the discipline documents as load-bearing.
    HandTagged,
    /// This CPU's kernel root, which is the boot identity map and runs under
    /// ASID 0 by construction, so the userspace ASID does not apply to it.
    KernelRoot,
    /// Read back out of `ttbr0_el1` or out of a shadow word: the ASID field is
    /// whatever was already installed rather than a fresh choice.
    ReadBack,
    /// Came in through the signature: the caller chose it.
    CallerBorne,
    /// An unaccounted value reaching a corridor word: it matched no arm above.
    Unaccounted,
}

impl Provenance {
    fn label(self) -> &'static str {
        match self {
            Provenance::Cleared => "cleared",
            Provenance::Normalised => "normalised",
            Provenance::HandTagged => "HAND-TAGGED",
            Provenance::KernelRoot => "kernel root",
            Provenance::ReadBack => "read back",
            Provenance::CallerBorne => "caller-borne",
            Provenance::Unaccounted => "UNACCOUNTED",
        }
    }

    fn is_accounted(self) -> bool {
        !matches!(
            self,
            Provenance::CallerBorne | Provenance::HandTagged | Provenance::Unaccounted
        )
    }
}

/// `body` with `//` line comments removed, so a comment that names a call the
/// code no longer makes cannot enter the census as a publish.
fn without_line_comments(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let visible = match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        };
        out.push_str(visible);
        out.push_str("\n");
    }
    out
}

/// The right-hand side of a plain `name = ...;` assignment in `body`.
///
/// `let_binding_rhs` alone misses the deferred-initialisation shape, which is
/// the one `sys_exec_aarch64` uses: `let previous_ttbr0;` on one line and
/// `previous_ttbr0 = read_ttbr0_for_exec();` in the block below it.
fn assignment_rhs(body: &str, name: &str) -> Option<String> {
    let needle = format!("{name} = ");
    let mut cursor = 0usize;
    while let Some(offset) = body[cursor..].find(&needle) {
        let at = cursor + offset;
        let preceding = body[..at].trim_end();
        let is_longer_identifier = body[..at]
            .chars()
            .last()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let is_binding = preceding.ends_with("let") || preceding.ends_with("mut");
        let is_comparison = preceding.ends_with('!')
            || preceding.ends_with('=')
            || preceding.ends_with('<')
            || preceding.ends_with('>');
        if !is_longer_identifier && !is_binding && !is_comparison {
            let rest = &body[at + needle.len()..];
            let end = rest.find(';').unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
        cursor = at + needle.len();
    }
    None
}

/// The expression `name` is bound to in `body`, by `let` or by assignment.
fn bound_expression(body: &str, name: &str) -> Option<String> {
    let_binding_rhs(body, name).or_else(|| assignment_rhs(body, name))
}

/// Functions declared in `source` whose body reads `ttbr0_el1` with an `mrs`.
/// A call to one of these is a register read-back however it is spelled.
fn register_reading_functions(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in declared_function_names(source) {
        let body = function_body(source, &name);
        if body.contains("mrs") && body.contains("ttbr0_el1") {
            out.push(name);
        }
    }
    out
}

/// The provenance `expression` carries on its own, if any.
fn expression_provenance(
    expression: &str,
    register_readers: &[String],
    dispatch_tag: (u64, u32),
) -> Option<Provenance> {
    if expression.contains("process_root_ttbr0(") {
        return Some(Provenance::Normalised);
    }
    // R157/ASID-05. Reaching the discipline's tag WITHOUT going through the
    // normaliser is the or-only shape, and it is scored as its own
    // disposition rather than accepted. Before this round the same predicate
    // returned an ACCOUNTED `DispatchTagged` here, which is why the census
    // read `ttbr0 | (1u64 << 48)` in `set_next_ttbr0_for_thread` as fine.
    if expression.contains("USER_ASID_TTBR0") || asid_tag(expression) == Some(dispatch_tag) {
        return Some(Provenance::HandTagged);
    }
    if expression.contains("kernel_cr3()") || expression.contains("kernel_ttbr0()") {
        return Some(Provenance::KernelRoot);
    }
    if expression.contains("next_cr3()")
        || expression.contains("saved_process_cr3()")
        || (expression.contains("mrs") && expression.contains("ttbr0_el1"))
    {
        return Some(Provenance::ReadBack);
    }
    for reader in register_readers {
        if calls_function(expression, reader) {
            return Some(Provenance::ReadBack);
        }
    }
    None
}

/// Where `operand` got its ASID field, followed through this function's own
/// bindings. Depth-limited for the same reason `traces_to_a_parameter` is: a
/// publish normalises or tags a value a step or two before publishing it.
fn value_provenance(
    signature: &str,
    body: &str,
    operand: &str,
    register_readers: &[String],
    dispatch_tag: (u64, u32),
) -> Provenance {
    let operand = operand.trim();
    if operand == "0" {
        return Provenance::Cleared;
    }
    if operand.is_empty() {
        return Provenance::Unaccounted;
    }
    if let Some(found) = expression_provenance(operand, register_readers, dispatch_tag) {
        return found;
    }

    let parameters = identifiers(signature);
    let mut frontier = vec![operand.to_string()];
    let mut seen = BTreeSet::new();
    let mut reached_a_parameter = false;
    // The sibling of R7-002 on `derivation_fetches_nothing`: a value the
    // function FETCHED is not a value its caller handed over, even when the
    // thing it fetched from came in through the signature.
    // `page_table.level_4_frame().start_address().as_u64()` bottoms out at a
    // parameter and is exactly the untagged root this ratchet exists to catch,
    // so any call in the derivation chain disqualifies the caller-borne
    // disposition.
    let mut fetched = false;
    for _ in 0..4 {
        let mut next = Vec::new();
        for name in std::mem::take(&mut frontier) {
            if !seen.insert(name.clone()) {
                continue;
            }
            match bound_expression(body, &name) {
                Some(expression) => {
                    if let Some(found) =
                        expression_provenance(&expression, register_readers, dispatch_tag)
                    {
                        return found;
                    }
                    if expression.contains('(') {
                        fetched = true;
                    }
                    next.extend(identifiers(&expression));
                }
                None => {
                    if parameters.contains(&name) {
                        reached_a_parameter = true;
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }

    if reached_a_parameter && !fetched {
        Provenance::CallerBorne
    } else {
        Provenance::Unaccounted
    }
}

/// One call that writes a corridor shadow word.
struct ShadowPublish {
    file: String,
    function: String,
    setter: String,
    operand: String,
    provenance: Provenance,
}

impl ShadowPublish {
    fn site(&self) -> String {
        format!("{}::{}", self.file, self.function)
    }

    fn disposition(&self) -> String {
        format!(
            "{}::{} {}({}) [{}]",
            self.file,
            self.function,
            self.setter,
            self.operand,
            self.provenance.label()
        )
    }
}

/// The calls in aarch64-scoped code under `kernel/src` that write a corridor
/// shadow word, with the provenance of the value each publishes.
/// claim-lint:ok: 17 of 17 at this head, printed by
/// `every_shadow_publish_has_an_accounted_asid -- --nocapture` in
/// docs/planning/green-program/aarch64-testing/serials/asid-ratchet/05-suite-green-with-census.txt
fn shadow_publish_census(
    sources: &[(String, String)],
    dispatch_tag: (u64, u32),
) -> Vec<ShadowPublish> {
    let mut out = Vec::new();
    for (file, name, raw_body) in aarch64_scoped_functions(sources) {
        let source = sources
            .iter()
            .find(|(candidate, _)| candidate == &file)
            .map(|(_, source)| source.as_str())
            .expect("the census walks sources it was handed");
        let register_readers = register_reading_functions(source);
        let signature = function_signature(source, &name);
        let body = without_line_comments(&raw_body);
        for setter in SHADOW_WRITERS {
            for operand in call_arguments(&body, setter) {
                if operand.is_empty() {
                    continue;
                }
                let provenance =
                    value_provenance(&signature, &body, &operand, &register_readers, dispatch_tag);
                out.push(ShadowPublish {
                    file: file.clone(),
                    function: name.clone(),
                    setter: setter.to_string(),
                    operand,
                    provenance,
                });
            }
        }
    }
    out
}

/// Caller-borne publishes whose callers do not resolve the provenance either,
/// as `caller -> callee(argument)` strings.
///
/// A publish that hands over a parameter has not decided the ASID; its callers
/// have. This walks them one level, which is the depth the shape needs: the
/// one caller-borne publish with a live caller at this head is
/// `restore_ttbr0_after_failed_exec`, whose caller captures the value from
/// `ttbr0_el1` itself.
fn unresolved_caller_borne(
    sources: &[(String, String)],
    census: &[ShadowPublish],
    dispatch_tag: (u64, u32),
) -> Vec<String> {
    let mut out = Vec::new();
    for publish in census {
        if publish.provenance != Provenance::CallerBorne {
            continue;
        }
        for (file, name, raw_body) in aarch64_scoped_functions(sources) {
            if name == publish.function {
                continue;
            }
            let source = sources
                .iter()
                .find(|(candidate, _)| candidate == &file)
                .map(|(_, source)| source.as_str())
                .expect("the census walks sources it was handed");
            let register_readers = register_reading_functions(source);
            let signature = function_signature(source, &name);
            let body = without_line_comments(&raw_body);
            for argument in call_arguments(&body, &publish.function) {
                let provenance =
                    value_provenance(&signature, &body, &argument, &register_readers, dispatch_tag);
                if !provenance.is_accounted() {
                    out.push(format!(
                        "{file}::{name} -> {}({argument}) [{}]",
                        publish.function,
                        provenance.label()
                    ));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The ASID tag the discipline names, parsed out of its own constant.
fn discipline_asid_tag(ttbr0: &str) -> (u64, u32) {
    let line = ttbr0
        .lines()
        .find(|line| line.contains("const USER_ASID_TTBR0"))
        .expect("the discipline must name the userspace ASID as a constant");
    asid_tag(line).unwrap_or_else(|| panic!("parse the discipline ASID tag from {line:?}"))
}

#[test]
fn every_shadow_publish_has_an_accounted_asid() {
    let sources = rust_sources_below("kernel/src");
    let tag = discipline_asid_tag(&repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs"));
    let census = shadow_publish_census(&sources, tag);

    assert!(
        census.len() >= 12,
        "the shadow-publish census reached only {} calls, so it is not covering the code it \
         claims to",
        census.len()
    );

    let dispositions: Vec<String> = census.iter().map(ShadowPublish::disposition).collect();
    eprintln!(
        "TTBR0 shadow-publish census ({} calls): {dispositions:#?}",
        dispositions.len()
    );

    // The classifier has to be able to tell these apart on the real tree, or
    // "no unaccounted publishes" would be a statement about a predicate that
    // does not fire. Each accounted provenance is present at this head, and
    // the census prints which call carries which.
    // claim-lint:ok: 4 of 4 accounted provenances appear among the 17
    // publishes printed in
    // docs/planning/green-program/aarch64-testing/serials/asid-ratchet/09-suite-green-after-r157.txt
    for expected in [
        Provenance::Cleared,
        Provenance::Normalised,
        Provenance::KernelRoot,
        Provenance::ReadBack,
    ] {
        assert!(
            census.iter().any(|publish| publish.provenance == expected),
            "no publish in the tree carries the {} provenance, so that arm of the classifier is \
             unexercised",
            expected.label()
        );
    }

    let mut unaccounted = Vec::new();
    for publish in &census {
        let in_the_discipline = publish.file == "kernel/src/arch_impl/aarch64/ttbr0.rs";
        let caller_borne = publish.provenance == Provenance::CallerBorne;
        // The discipline is where normalisation is DEFINED. A publish inside
        // it that hands the corridor a value its caller chose is the
        // pre-9e877486 shape exactly, and it may not push the ASID decision
        // back onto the routed call sites.
        if !publish.provenance.is_accounted() && !caller_borne
            || (in_the_discipline && caller_borne)
        {
            unaccounted.push(publish.disposition());
        }
    }
    assert!(
        unaccounted.is_empty(),
        "these publishes hand the syscall return corridor a value with no ASID provenance, and \
         the corridor installs the word verbatim: {unaccounted:#?}"
    );

    let unresolved = unresolved_caller_borne(&sources, &census, tag);
    assert!(
        unresolved.is_empty(),
        "these callers hand a shadow publish a value with no ASID provenance: {unresolved:#?}"
    );
}

#[test]
fn the_publication_census_catches_an_untagged_root() {
    let tag = (1, 48);
    let with = |body: &str| {
        vec![(
            "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
            body.to_string(),
        )]
    };

    // Leg 1: a new publish site handing the corridor a bare page-table root.
    let raw = "fn publish_a_root(page_table: &PageTable) {\n\
        let root = page_table.level_4_frame().start_address().as_u64();\n\
        unsafe { Aarch64PerCpu::set_next_cr3(root); }\n\
    }\n";
    let census = shadow_publish_census(&with(raw), tag);
    let caught: Vec<String> = census
        .iter()
        .filter(|publish| publish.provenance == Provenance::Unaccounted)
        .map(ShadowPublish::site)
        .collect();
    assert_eq!(
        caught,
        vec!["kernel/src/arch_impl/aarch64/invented.rs::publish_a_root".to_string()],
        "an untagged root reaching a shadow store has to be caught"
    );

    // Leg 2: the same site, normalised. The census must accept it: without
    // this arm, a predicate that rejected every publish would pass leg 1.
    // claim-lint:ok: 5 legs in this test, and the in-tree mutation that
    // reverts the discipline is recorded in
    // docs/planning/green-program/aarch64-testing/serials/asid-ratchet/01-structural-anti-vacuity-raw-adopt.txt
    let normalised = raw.replace(
        "Aarch64PerCpu::set_next_cr3(root)",
        "Aarch64PerCpu::set_next_cr3(process_root_ttbr0(root))",
    );
    assert!(
        shadow_publish_census(&with(&normalised), tag)
            .iter()
            .all(|publish| publish.provenance.is_accounted()),
        "normalising the operand has to clear it"
    );

    // Leg 3: the pre-9e877486 discipline shape -- the operand is the caller's,
    // and the discipline publishes it unchanged.
    let discipline = "fn adopt_process_ttbr0(ttbr0_value: u64) {\n\
        unsafe {\n\
            super::percpu::Aarch64PerCpu::set_saved_process_cr3(ttbr0_value);\n\
            super::percpu::Aarch64PerCpu::set_next_cr3(0);\n\
        }\n\
    }\n";
    let sources = vec![(
        "kernel/src/arch_impl/aarch64/ttbr0.rs".to_string(),
        discipline.to_string(),
    )];
    let census = shadow_publish_census(&sources, tag);
    assert!(
        census.iter().any(|publish| publish.provenance == Provenance::CallerBorne
            && publish.function == "adopt_process_ttbr0"),
        "the discipline publishing its raw operand is caller-borne, which the census fails on \
         inside the discipline module"
    );

    // Leg 4: a caller-borne publish OUTSIDE the discipline is resolved at its
    // callers, and a caller handing over a raw root is what fails.
    let primitive = "fn install_and_publish(ttbr0: u64) {\n\
        unsafe { Aarch64PerCpu::set_saved_process_cr3(ttbr0); }\n\
    }\n\
    fn a_caller(page_table: &PageTable) {\n\
        let root = page_table.level_4_frame().start_address().as_u64();\n\
        install_and_publish(root);\n\
    }\n";
    let sources = with(primitive);
    let census = shadow_publish_census(&sources, tag);
    assert!(
        census
            .iter()
            .any(|publish| publish.provenance == Provenance::CallerBorne),
        "a publish of a parameter is caller-borne"
    );
    assert_eq!(
        unresolved_caller_borne(&sources, &census, tag),
        vec![
            "kernel/src/arch_impl/aarch64/invented.rs::a_caller -> install_and_publish(root) \
             [UNACCOUNTED]"
                .to_string()
        ],
        "the caller that chose the untagged root is the site that has to be named"
    );

    // Leg 5: the same caller, capturing the value from the register instead.
    let read_back = primitive.replace(
        "let root = page_table.level_4_frame().start_address().as_u64();",
        "let root = read_ttbr0();",
    ) + "fn read_ttbr0() -> u64 {\n\
        let value: u64;\n\
        unsafe { core::arch::asm!(\"mrs {}, ttbr0_el1\", out(reg) value); }\n\
        value\n\
    }\n";
    let sources = with(&read_back);
    let census = shadow_publish_census(&sources, tag);
    assert!(
        unresolved_caller_borne(&sources, &census, tag).is_empty(),
        "a caller that hands over what the register already held is resolved"
    );
}

#[test]
fn the_shadow_setters_feed_the_runtime_census() {
    // The census above reads source shapes and stops there. What says the
    // shipped kernel publishes a tagged root is the runtime counter, and it
    // can only say so while both writes still go through it.
    // claim-lint:ok: 2 of 2 per-CPU setters are checked below; the 3 boots
    // that read the counter are in
    // docs/planning/green-program/aarch64-testing/serials/asid-ratchet/04-prod-boot1.txt
    // and its 2 siblings
    let percpu = repo_text("kernel/src/arch_impl/aarch64/percpu.rs");
    for setter in SHADOW_WRITERS {
        let body = function_body(&percpu, setter);
        let write = body
            .find("percpu_write_u64")
            .unwrap_or_else(|| panic!("{setter} must write the per-CPU word"));
        let count = body.find("note_shadow_publish").unwrap_or_else(|| {
            panic!("{setter} writes a corridor shadow word without counting what it publishes")
        });
        assert!(
            count < write,
            "{setter} must count the value it is about to publish, not one it has published"
        );
    }

    let ttbr0 = repo_text("kernel/src/arch_impl/aarch64/ttbr0.rs");
    let counter = function_body(&ttbr0, "note_shadow_publish");
    assert!(
        counter.contains("USER_ASID_TTBR0"),
        "the runtime census must score the ASID field against the discipline's own constant"
    );
    assert!(
        counter.contains("kernel_ttbr0()"),
        "the runtime census must exempt the kernel root, which is ASID 0 by construction"
    );
    for forbidden in ["lock()", "serial_println!", "log::", "format!", "alloc::"] {
        assert!(
            !counter.contains(forbidden),
            "the runtime census runs where the shadow words are written and must not use \
             {forbidden}"
        );
    }

    let emitter = function_body(&ttbr0, "emit_asid_census");
    assert!(
        emitter.contains("[TTBR0_ASID_CENSUS:untagged="),
        "the census marker the gates pin has to be the one the kernel prints"
    );
}

/// Score `serial` with `gate`'s OWN verdict code, without booting.
///
/// Each aarch64 gate exposes a scoring-only entry point through an environment
/// variable; setting it makes the script skip the QEMU boot and run its verdict
/// block against the named serial. The `contains` check is a guard, not the
/// assertion: if the entry point were removed, invoking the script here would
/// boot QEMU out of a unit test, so this fails first instead.
fn score_with_gate(gate: &str, variable: &str, serial: &Path) -> (bool, String) {
    let script = repo_text(gate);
    assert!(
        script.contains(variable),
        "{gate} has no {variable} scoring-only entry point, so its verdict rules cannot be run \
         from a test -- and invoking it without one would boot QEMU"
    );
    let output = std::process::Command::new("bash")
        .arg(repo_root().join(gate))
        .env(variable, serial)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|error| panic!("run {gate} in scoring-only mode: {error}"));
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

/// `serial` with every census line rewritten to `replacement`, or deleted when
/// there is none.
/// claim-lint:ok: 2 of 2 uses of this helper are legs C and D below
fn rewrite_census_lines(serial: &str, replacement: Option<&str>) -> String {
    let mut out = String::new();
    for line in serial.lines() {
        if line.contains("[TTBR0_ASID_CENSUS:") {
            if let Some(text) = replacement {
                out.push_str(text);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[test]
fn both_aarch64_gates_fail_on_an_untagged_publish() {
    // R157/ASID-01. What stood here asserted that each gate script CONTAINS
    // three pattern strings. That stays true of a script whose every census
    // assertion has been deleted and whose variable definitions remain, and it
    // was demonstrated: with the assertions removed the strict gate scored a
    // serial reporting untagged=3 as PASS while this test stayed green. So the
    // gates are RUN now, on a serial each was recorded green on and on three
    // mutations of it, and the exit status is the measurement.
    // claim-lint:ok: 4 legs against each of the 2 gates; the equivalent legs
    // run by hand against 1 of them are in
    // docs/planning/green-program/aarch64-testing/serials/asid-ratchet/07-strict-score-legs.txt
    let gates = [
        (
            "docker/qemu/run-aarch64-boot-test-strict.sh",
            "BREENIX_STRICT_SCORE_ONLY",
            "docs/planning/green-program/aarch64-testing/serials/slice3e/01-strict-boot1-serial.txt",
        ),
        (
            "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
            "BREENIX_PROD_SCORE_ONLY",
            "docs/planning/green-program/aarch64-testing/serials/slice3e/02-prod-boot1-serial.txt",
        ),
    ];

    let scratch =
        std::env::temp_dir().join(format!("breenix-asid-gate-legs-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("create the scratch directory for the gate legs");

    for (gate, variable, baseline) in gates {
        let serial = repo_text(baseline);
        assert!(
            serial.contains("[TTBR0_ASID_CENSUS:untagged=0:tagged="),
            "{baseline} is the green baseline for {gate} and has to carry the census it is the \
             baseline for"
        );
        let leg = |name: &str, body: &str| {
            let path = scratch.join(format!("{}-{name}.txt", gate.replace('/', "_")));
            fs::write(&path, body).expect("write a gate leg serial");
            score_with_gate(gate, variable, &path)
        };

        // Leg A. Anti-vacuity for the three below: a gate that rejected every
        // serial would satisfy them without scoring anything.
        // claim-lint:ok: 1 of 4 legs is this one, and it is what keeps the other 3 from
        // being satisfied by a gate that rejects everything
        let (passed, output) = leg("green", &serial);
        assert!(
            passed,
            "{gate} has to pass the serial it was recorded green on, or the failing legs below \
             say nothing: {output}"
        );

        // Leg B. One census line reports a non-zero untagged publish. That is
        // the defect class the counter exists for.
        // claim-lint:ok: 1 of 13 census lines in the baseline serial is rewritten here
        let (passed, output) = leg(
            "untagged",
            &serial.replacen(
                "[TTBR0_ASID_CENSUS:untagged=0:tagged=",
                "[TTBR0_ASID_CENSUS:untagged=3:tagged=",
                1,
            ),
        );
        assert!(
            !passed,
            "{gate} passed a serial reporting an untagged process-root publish: {output}"
        );
        assert!(
            output.contains("untagged"),
            "{gate} failed the untagged serial, but for some other reason: {output}"
        );

        // Leg C. The census never printed. A gate that only fails on a
        // non-zero reading is satisfied by a kernel that stopped reporting.
        // claim-lint:ok: this leg deletes 13 of 13 census lines in each baseline serial
        let (passed, output) = leg("missing", &rewrite_census_lines(&serial, None));
        assert!(
            !passed,
            "{gate} passed a serial with no census line at all: {output}"
        );

        // Leg D. The census printed, having counted no process-root publish.
        // untagged=0 then says nothing, and a dead counter reads the same.
        // claim-lint:ok: this leg rewrites 13 of 13 census lines in each baseline serial
        // to tagged=0, which the
        // gate rejects with its third assertion
        let (passed, output) = leg(
            "vacuous",
            &rewrite_census_lines(
                &serial,
                Some("[TTBR0_ASID_CENSUS:untagged=0:tagged=0:kernel=0:cleared=0]"),
            ),
        );
        assert!(
            !passed,
            "{gate} passed a serial whose census counted no process-root publish: {output}"
        );
    }

    fs::remove_dir_all(&scratch).ok();
}

// ---------------------------------------------------------------------------
// One place constructs the ASID tag (R157 / ASID-05)
// ---------------------------------------------------------------------------
//
// `process_root_ttbr0`'s doc comment states the rule: the ASID field is
// REPLACED rather than or-ed into, because an or-only tag preserves a foreign
// ASID a caller happened to hand over. Until this round three sites spelled the
// tag themselves anyway -- `set_next_ttbr0_for_thread` with
// `ttbr0 | (1u64 << 48)`, `launch_init_from_elf` with
// `ttbr0_phys | (1u64 << 48)`, and the aarch64
// `switch_to_process_page_table` carrying the register's own ASID field
// forward -- and nothing failed, because the publish census scored a
// hand-spelled tag as an ACCOUNTED disposition.
// claim-lint:ok: 3 of 3 or-only sites in the tree at the previous head are listed here,
// and section 14 of docs/planning/green-program/aarch64-testing/TTBR0-ASID-
// RATCHET-2026-09-05.md records the change to each
//
// Disclosed narrowing: this predicate reads the tag's two spellings, the
// discipline's constant and a shift that parses to the same tag. It does NOT
// reach a hand-managed ASID field spelled with the `0xFFFF_0000_0000_0000`
// mask, because that literal is also this kernel's HHDM base and appears on a
// dozen unrelated lines. The one site in the tree with that spelling was
// removed in this round rather than left for a predicate that cannot see it.
// claim-lint:ok: 12 of the 13 `0xFFFF_0000_0000_0000` occurrences under
// `kernel/src` at the previous head are HHDM or kernel-base constants; the
// 13th was `kernel/src/memory/process_memory.rs::switch_to_process_page_table`.

/// The lines of `source` that CONSTRUCT the userspace ASID tag -- naming the
/// discipline's constant, or spelling a shift that parses to the same tag --
/// and or it into something. `//` comments are stripped first, so prose naming
/// the old spelling is not a finding.
fn asid_tag_constructions(source: &str, discipline_tag: (u64, u32)) -> Vec<String> {
    let mut out = Vec::new();
    for (index, line) in without_line_comments(source).lines().enumerate() {
        if !line.contains('|') {
            continue;
        }
        if line.contains("USER_ASID_TTBR0") || asid_tag(line) == Some(discipline_tag)
        {
            let mut record = String::new();
            record.push_str(&(index + 1).to_string());
            record.push_str(": ");
            record.push_str(line.trim());
            out.push(record);
        }
    }
    out
}

#[test]
fn the_asid_tag_is_constructed_in_one_place() {
    let discipline_file = "kernel/src/arch_impl/aarch64/ttbr0.rs";
    let tag = discipline_asid_tag(&repo_text(discipline_file));

    let mut elsewhere = Vec::new();
    for (file, source) in rust_sources_below("kernel/src") {
        if file == discipline_file {
            continue;
        }
        for line in asid_tag_constructions(&source, tag) {
            let mut record = file.clone();
            record.push_str(": ");
            record.push_str(&line);
            elsewhere.push(record);
        }
    }
    assert!(
        elsewhere.is_empty(),
        "these sites build the userspace ASID tag themselves instead of calling \
         `process_root_ttbr0`, which is the or-only form the discipline's own doc comment \
         refuses -- a foreign ASID in the operand survives it: {elsewhere:#?}"
    );

    // Anti-vacuity on the real tree: the discipline's file DOES construct the
    // tag, so "nowhere else does" is a statement about a predicate that fires.
    // claim-lint:ok: 1 of 1 file constructing the tag at this head is the discipline's
    // own, which is what this assertion reads
    assert!(
        !asid_tag_constructions(&repo_text(discipline_file), tag).is_empty(),
        "the discipline has to construct the tag somewhere, or this census reads nothing"
    );
}

#[test]
fn the_tag_census_reads_the_or_and_not_the_comparison() {
    // Anti-vacuity for the census above, on synthetic sources: the predicate
    // has to see the or-only spelling, and has to stay quiet on the two shapes
    // that mention the tag without constructing it.
    let tag = (1, 48);
    assert_eq!(
        asid_tag_constructions("let value = root | (1u64 << 48);\n", tag).len(),
        1,
        "the shift spelling of the tag, or-ed in, is what this catches"
    );
    assert_eq!(
        asid_tag_constructions("let value = root | USER_ASID_TTBR0;\n", tag).len(),
        1,
        "the constant spelling, or-ed in, is the same finding"
    );
    assert!(
        asid_tag_constructions("let value = process_root_ttbr0(root);\n", tag).is_empty(),
        "routing through the normaliser is the accepted spelling, not a finding"
    );
    assert!(
        asid_tag_constructions("if value & MASK == USER_ASID_TTBR0 {}\n", tag).is_empty(),
        "comparing against the tag is not constructing it"
    );
    assert!(
        asid_tag_constructions("// let value = root | (1u64 << 48);\n", tag).is_empty(),
        "prose naming the old spelling is not a finding"
    );
    assert!(
        asid_tag_constructions("let value = root | (2u64 << 48);\n", tag).is_empty(),
        "a different ASID is a different question; this census is about THIS tag"
    );
}

#[test]
fn the_publish_census_tells_a_replaced_tag_from_an_or_ed_one() {
    // R157/ASID-05 anti-vacuity. The two spellings differ in exactly one way
    // that matters -- whether a foreign ASID in the operand survives -- and
    // before this round the publish census scored them identically, as an
    // ACCOUNTED disposition named after the dispatch path.
    let tag = (1, 48);
    let with = |body: &str| {
        vec![(
            "kernel/src/arch_impl/aarch64/invented.rs".to_string(),
            body.to_string(),
        )]
    };
    let or_ed = concat!(
        "fn publish_a_root(page_table: &PageTable) {\n",
        "    let root = page_table.level_4_frame().start_address().as_u64();\n",
        "    unsafe { Aarch64PerCpu::set_next_cr3(root | (1u64 << 48)); }\n",
        "}\n",
    );
    let census = shadow_publish_census(&with(or_ed), tag);
    let dispositions: Vec<String> = census.iter().map(ShadowPublish::disposition).collect();
    assert_eq!(
        census.len(),
        1,
        "the leg has to produce exactly the one publish it is about: {dispositions:#?}"
    );
    assert_eq!(
        census[0].provenance,
        Provenance::HandTagged,
        "an or-only tag is not a normalisation and must not be accounted: {dispositions:#?}"
    );
    assert!(!census[0].provenance.is_accounted());

    let replaced = or_ed.replace(
        "set_next_cr3(root | (1u64 << 48))",
        "set_next_cr3(process_root_ttbr0(root))",
    );
    let census = shadow_publish_census(&with(&replaced), tag);
    assert!(
        census
            .iter()
            .all(|publish| publish.provenance == Provenance::Normalised),
        "replacing the field through the normaliser is the accepted spelling"
    );
}

// ---------------------------------------------------------------------------
// The census's completeness premise, enforced (R157 / ASID-03)
// ---------------------------------------------------------------------------
//
// `SHADOW_WRITERS` is keyed on two function names, and the comment above it
// claims "a new publish site is reached because it has to call one of them".
// Nothing pinned that. A third function writing `PERCPU_NEXT_CR3_OFFSET` with
// `percpu_write_u64` directly would evade the structural census, which is keyed
// on the setter names, AND the runtime counter, which lives inside them. That
// `percpu_write_u64` is module-private today is a fact about today's tree, not
// a ratchet -- the module it is private to is the one that would host the third
// writer.
// claim-lint:ok: 0 of 32 tests at the previous head counted writers of either offset;
// the mutation leg is the_writer_census_catches_a_second_writer below
//
// Disclosed narrowing: this counts functions that NAME the offset constants. A
// write that computed the offset arithmetically, or that went through a raw
// pointer without naming the constant, is outside it -- as are the 2 assembly
// stores separately accounted in the census comment in `ttbr0.rs`.

/// Every function under `kernel/src` whose body names `offset`, as
/// `file::function` paired with its comment-stripped body.
/// claim-lint:ok: 2 of 2 offsets reach 2 of 2 functions each at this head, both in
/// kernel/src/arch_impl/aarch64/percpu.rs
fn functions_naming(sources: &[(String, String)], offset: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (file, source) in sources {
        // A function whose body names the offset is in a file whose text names
        // it, so the walk narrows to those files first. That keeps this census
        // off sources whose declarations `function_body` cannot parse -- a
        // `fn` named inside a macro invocation, for one -- without exempting
        // anything it would otherwise have reached.
        if !source.contains(offset) {
            continue;
        }
        // `kernel/src/arch_impl/x86_64/constants.rs` defines constants with
        // these same two names. They are a different module's offsets into a
        // different per-CPU block, reached only from x86_64-scoped code, and
        // counting them here would mean this census failed on a namesake. The
        // aarch64 constants are `pub`, so the walk is otherwise kernel-wide
        // rather than confined to `arch_impl/aarch64/`: a shared file that
        // imported and wrote them would be counted.
        if file.contains("/x86_64/") {
            continue;
        }
        for name in declared_function_names(source) {
            let body = without_line_comments(function_body(source, &name));
            if body.contains(offset) {
                let mut site = file.clone();
                site.push_str("::");
                site.push_str(&name);
                out.push((site, body));
            }
        }
    }
    out
}

/// The censused functions in `touching` that WRITE `offset` through
/// `percpu_write_u64`.
fn offset_writers(touching: &[(String, String)], offset: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (site, body) in touching {
        for argument in call_arguments(body, "percpu_write_u64") {
            let end = argument.find(',').unwrap_or(argument.len());
            if argument[..end].trim() == offset {
                out.push(site.clone());
                break;
            }
        }
    }
    out
}

#[test]
fn each_corridor_shadow_word_has_exactly_one_writer() {
    let sources = rust_sources_below("kernel/src");
    let offsets = [
        "PERCPU_SAVED_PROCESS_CR3_OFFSET",
        "PERCPU_NEXT_CR3_OFFSET",
    ];
    for offset in offsets {
        let touching = functions_naming(&sources, offset);
        let mut names = Vec::new();
        for (site, _) in &touching {
            names.push(site.clone());
        }

        let writers = offset_writers(&touching, offset);
        assert_eq!(
            writers.len(),
            1,
            "{offset} has to have exactly one writer, or both the structural census \
             (keyed on the setter NAMES) and the runtime counter (living inside them) can be \
             walked around: {writers:#?} out of {names:#?}"
        );

        let setter = writers[0]
            .rsplit("::")
            .next()
            .expect("a censused function has a name");
        assert!(
            SHADOW_WRITERS.contains(&setter),
            "{offset} is written by {setter}, which is not one of the setters the \
             publish census and the runtime counter are keyed on: {SHADOW_WRITERS:?}"
        );

        let mut readers = Vec::new();
        for (site, body) in &touching {
            if site != &writers[0] && body.contains("percpu_read_u64") {
                readers.push(site.clone());
            }
        }
        assert_eq!(
            names.len(),
            writers.len() + readers.len(),
            "some function names {offset} while neither reading nor writing it through \
             the per-CPU accessors, which is the shape that would evade both censuses: \
             {names:#?}"
        );
    }
}

#[test]
fn the_writer_census_catches_a_second_writer() {
    // Anti-vacuity for the count above, on synthetic sources rather than on the
    // tree: the predicate has to see a second writer where there is one, or an
    // exactly-one count is a statement about a census that reaches nothing.
    // claim-lint:ok: 1 of 2 synthetic sources in this leg has one writer and the other
    // has two, which is the whole contract
    let offset = "PERCPU_NEXT_CR3_OFFSET";
    let one_writer = concat!(
        "pub unsafe fn set_next_cr3(val: u64) {\n",
        "    percpu_write_u64(PERCPU_NEXT_CR3_OFFSET, val);\n",
        "}\n",
    );
    let second_writer = concat!(
        "pub unsafe fn arm_the_corridor(val: u64) {\n",
        "    percpu_write_u64(PERCPU_NEXT_CR3_OFFSET, val);\n",
        "}\n",
    );
    let path = "kernel/src/arch_impl/aarch64/percpu.rs".to_string();

    let one = vec![(path.clone(), one_writer.to_string())];
    assert_eq!(offset_writers(&functions_naming(&one, offset), offset).len(), 1);

    let mut both = one_writer.to_string();
    both.push_str(second_writer);
    let two = vec![(path, both)];
    assert_eq!(
        offset_writers(&functions_naming(&two, offset), offset).len(),
        2,
        "a second function writing the offset directly has to be counted, or the \
         completeness premise of SHADOW_WRITERS is unenforced"
    );
}
