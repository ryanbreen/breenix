//! Ratchet: every hand-written interrupt/syscall entry point clears the
//! direction flag before it can reach a `rep`-prefixed string operation or a
//! call into Rust (#737).
//! claim-lint:ok: "every" here ranges over the census this file derives, not
//! over the tree at large -- 3 of 3 `.asm` files under the two entry
//! directories today -- and the count is recomputed and printed by
//! `the_asm_entry_census_is_not_vacuous` on every run rather than frozen here.
//!
//! # Why this exists
//!
//! An x86-64 interrupt gate clears IF. It does **not** clear DF. So a ring-3
//! thread preempted while DF=1 -- which happens for real, inside the
//! `std ... cld` backward arm of userspace `memmove` -- enters the handler
//! with DF=1, and every `rep`-prefixed string operation the handler then runs
//! copies downward. #737's specimen is exactly that: the timer ISR reached a
//! `log::trace!`, whose `log_impl` calls `log::RecordBuilder::new`, whose
//! 128-byte struct return is a `rep movsq` into an out-pointer above
//! `log_impl`'s spill of the `&'static Location`; backwards, that copy
//! overwrote the spill and `Location::file` faulted dereferencing it.
//! claim-lint:ok: "every rep-prefixed string operation runs backwards" is the
//! definition of DF=1 on x86-64, not a survey of this tree; the specimen and
//! its disassembly belong to #737's joined RCA record.
//!
//! Nothing in this tree asserted DF=0 on kernel entry, at build time or at
//! run time, so the omission was invisible to every gate for the whole life
//! of the file. This test is that assertion.
//! claim-lint:ok: measured in the round that added this file -- `grep -rniE
//! 'direction_flag|DIRECTION FLAG|DF flag' kernel/src tests scripts docker`
//! returns 2 hits outside this file, and 0 of 2 is an assertion or a check:
//! both are source comments sitting on the two `cld` instructions that already
//! exist (`kernel/src/syscall/entry.asm:49`,
//! `kernel/src/interrupts/breakpoint_entry.asm:59`). #737.
//!
//! # The census (no literal name list -- the #549, #551, #527-r1 lesson)
//!
//! Entry points are derived by SHAPE, in three steps:
//!
//! 1. **Which files.** Every `*.asm` under `kernel/src/interrupts/` and
//!    `kernel/src/syscall/`, recursively. A file counts as reaching the
//!    kernel image if `kernel/build.rs` names it in a `nasm` invocation, or
//!    if any `kernel/src/**/*.rs` pulls it in with `global_asm!` /
//!    `include_str!` / `include!`. A `.asm` file that neither route reaches
//!    is not in the image and is reported, not asserted on.
//!    claim-lint:ok: 3 of 3 `.asm` files anywhere under `kernel/src` sit in
//!    those two directories, and 3 of 3 are named by `kernel/build.rs`; both
//!    counts are recomputed per run, not frozen here. #737.
//!
//! 2. **Which symbols in those files.** Every `global <symbol>` directive.
//!    claim-lint:ok: the directive census is exactly what `global_symbols`
//!    computes, and `the_asm_entry_census_is_not_vacuous` fails if it yields
//!    an empty set. #737.
//!
//! 3. **Which of those symbols are hardware-entered.** A symbol is EXEMPT
//!    only when the Rust tree declares it as a function *taking parameters*
//!    (`fn syscall_return_to_userspace(user_rip: u64, ...)`), because that is
//!    a block entered by an ordinary `call` from compiled Rust, where the
//!    SysV ABI already guarantees DF=0 at entry. Everything else is POLICED:
//!    a symbol installed into the IDT or an MSR (`fn timer_interrupt_entry();`
//!    plus `timer_interrupt_entry as u64` at the install site), and any global
//!    symbol Rust does not describe at all. The default is "policed", so a new
//!    hand-written entry added later is covered by construction rather than by
//!    somebody remembering to edit a list.
//!    claim-lint:ok: the SysV "DF clear at function entry" rule is the psABI's,
//!    not a measurement of this tree. What is measured is which symbols it
//!    exempts, printed per run by `policed_entries`: 1 of 4 global symbols
//!    today. #737.
//!
//! # What is exempt, and why that is not a hole
//!
//! The kernel's OTHER interrupt entry points are `extern "x86-interrupt" fn`
//! handlers. Those are not hand-written: LLVM's `x86_intrcc` calling
//! convention emits a `cld` in the prologue itself, unprompted -- which is
//! also the cleanest available confirmation that the hardware does not clear
//! DF on interrupt entry. They are censused and reported by
//! `llvm_generated_x86_interrupt_handlers_are_censused_and_exempt` below so
//! the exemption is visible and countable, but they are not asserted on,
//! because the property is a fact about the compiler rather than about this
//! tree's source text.
//!
//! # Disclosed limits of the check
//!
//! * It is a LEXICAL order check over the entry's own region, not a
//!   control-flow analysis: it requires the first `cld` to appear before the
//!   first `call` and before the first string instruction, reading top to
//!   bottom from the entry label. A `cld` placed on a branch the entry can
//!   skip would satisfy this test and not the property. All three policed
//!   entries in the tree today are straight-line-then-merge shapes where the
//!   two coincide.
//!   claim-lint:ok: 3 of 3 policed entries read that way -- the two existing
//!   `cld` sites (`kernel/src/syscall/entry.asm:50`,
//!   `kernel/src/interrupts/breakpoint_entry.asm:60`) both sit on the merge
//!   point after their entry's swapgs test, and the third policed entry
//!   contains no `cld` at all, which is this ratchet's finding. #737.
//!
//! * Comment stripping cuts each line at its first `;`. NASM string literals
//!   containing a `;` would be mis-cut, and there are none in these files.
//!   claim-lint:ok: 2 of 2 double-quote characters across the 3 `.asm` files
//!   sit inside NASM comments (`kernel/src/syscall/entry.asm:118` and `:140`),
//!   so 0 of 3 files carry a string literal at all; and a mis-cut would make
//!   this check more lenient, never more strict. #737.
//!
//! * The Rust-declaration probe in step 3 reads EVERY `fn <symbol>(`
//!   occurrence under `kernel/src`, not only the first, and treats a symbol as
//!   hardware-entered when any one of them declares no parameters. That is
//!   fail-closed against a tree that declares the same symbol twice with
//!   different signatures.
//!   claim-lint:ok: 7 of 7 declarations of the 4 global symbols were measured
//!   by grep in the round that added this file; 6 of the 7 take no parameters
//!   and the 7th (`syscall_return_to_userspace`) takes three, so the
//!   fail-closed rule and a first-match rule agree today. #737.
//!
//! Host-side only: a text read of the tree, no kernel build and no QEMU boot.
//! Run: `cargo test --test entry_point_df_structure`.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| panic!("read repository file {}", path.display()))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Each regular file under `root`, recursively, sorted.
fn discover_files(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("read repository directory {}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|_| panic!("read entry in {}", directory.display()))
                .path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    if root.is_dir() {
        visit(root, &mut files);
    }
    files.sort();
    files
}

fn files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    discover_files(root)
        .into_iter()
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some(extension))
        .collect()
}

/// Step 1 of the census: hand-written assembly under the two entry
/// directories.
fn entry_asm_files() -> Vec<PathBuf> {
    let mut files = files_with_extension(&repo_root().join("kernel/src/interrupts"), "asm");
    files.extend(files_with_extension(
        &repo_root().join("kernel/src/syscall"),
        "asm",
    ));
    files.sort();
    files
}

/// A file reaches the kernel image if the kernel's build script assembles it,
/// or some kernel source pulls it in textually.
fn reaches_kernel_image(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .expect("assembly file has a name");

    let build_script = read_text(&repo_root().join("kernel/build.rs"));
    if build_script.contains(file_name) {
        return true;
    }

    files_with_extension(&repo_root().join("kernel/src"), "rs")
        .into_iter()
        .any(|source| {
            let text = read_text(&source);
            text.contains(file_name)
                && (text.contains("global_asm!")
                    || text.contains("include_str!")
                    || text.contains("include!"))
        })
}

/// Everything after the first `;` on a line is a NASM comment.
fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn first_token(line: &str) -> String {
    strip_comment(line)
        .trim()
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Step 2 of the census: `global <symbol>` directives.
fn global_symbols(text: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in text.lines() {
        let stripped = strip_comment(line);
        let stripped = stripped.trim();
        let rest = match stripped.strip_prefix("global ") {
            Some(rest) => rest,
            None => continue,
        };
        for candidate in rest.trim().split(|c: char| c.is_whitespace() || c == ':') {
            if !candidate.is_empty() {
                symbols.push(candidate.to_string());
                break;
            }
        }
    }
    symbols.sort();
    symbols.dedup();
    symbols
}

/// The parameter list of each Rust declaration of `symbol` under
/// `kernel/src`. An empty string means "declared, takes no parameters".
fn rust_declared_parameter_lists(symbol: &str) -> Vec<String> {
    let needle = format!("fn {}(", symbol);
    let mut lists = Vec::new();
    for source in files_with_extension(&repo_root().join("kernel/src"), "rs") {
        let text = read_text(&source);
        let mut cursor = 0usize;
        while let Some(offset) = text[cursor..].find(&needle) {
            let open = cursor + offset + needle.len();
            match text[open..].find(')') {
                Some(close) => {
                    lists.push(text[open..open + close].trim().to_string());
                    cursor = open + close;
                }
                None => {
                    cursor = open;
                }
            }
        }
    }
    lists
}

/// Reported, not asserted on: whether some Rust source takes the symbol's
/// address, which is how an entry is installed into the IDT or an MSR.
fn rust_takes_address(symbol: &str) -> bool {
    let as_u64 = format!("{} as u64", symbol);
    let as_usize = format!("{} as usize", symbol);
    files_with_extension(&repo_root().join("kernel/src"), "rs")
        .into_iter()
        .any(|source| {
            let text = read_text(&source);
            text.contains(&as_u64) || text.contains(&as_usize)
        })
}

/// Step 3 of the census, fail-closed: policed unless the tree declares the
/// symbol and each declaration found takes parameters.
fn is_hardware_entered(symbol: &str) -> bool {
    let lists = rust_declared_parameter_lists(symbol);
    if lists.is_empty() {
        return true;
    }
    lists.iter().any(|parameters| parameters.is_empty())
}

/// A label definition at column 0, e.g. `syscall_entry:`. NASM local labels
/// start with `.` and are deliberately not matched: they belong to the
/// enclosing global symbol's region.
fn label_at(line: &str) -> Option<String> {
    let stripped = strip_comment(line);
    if stripped.starts_with(char::is_whitespace) {
        return None;
    }
    let trimmed = stripped.trim_end();
    let name = trimmed.strip_suffix(':')?;
    if name.is_empty() {
        return None;
    }
    let mut characters = name.chars();
    let first = characters.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// The lines belonging to `symbol`: from its label to the next global label,
/// or to end of file. The returned index is the 1-based source line number.
fn region_of(text: &str, symbol: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| label_at(line).as_deref() == Some(symbol));
    let start = match start {
        Some(index) => index,
        None => return Vec::new(),
    };
    let mut region = Vec::new();
    for (offset, line) in lines[start + 1..].iter().enumerate() {
        if label_at(line).is_some() {
            break;
        }
        region.push((start + offset + 2, line.to_string()));
    }
    region
}

fn is_rep_prefix(token: &str) -> bool {
    matches!(token, "rep" | "repe" | "repz" | "repne" | "repnz")
}

fn is_string_mnemonic(token: &str) -> bool {
    let stem = token.trim_end_matches(['b', 'w', 'd', 'q']);
    matches!(
        stem,
        "movs" | "stos" | "lods" | "cmps" | "scas" | "ins" | "outs"
    )
}

/// The finding, if any: the entry reaches a `call` or a string operation with
/// no preceding `cld`.
fn missing_cld(text: &str, symbol: &str) -> Option<String> {
    for (line_number, line) in region_of(text, symbol) {
        let token = first_token(&line);
        if token == "cld" {
            return None;
        }
        if token == "call" {
            return Some(format!(
                "line {}: reaches `{}` with no preceding cld",
                line_number,
                strip_comment(&line).trim()
            ));
        }
        if is_rep_prefix(&token) || is_string_mnemonic(&token) {
            return Some(format!(
                "line {}: reaches string operation `{}` with no preceding cld",
                line_number,
                strip_comment(&line).trim()
            ));
        }
    }
    None
}

/// The full derived set: the (file, symbol) pairs this ratchet asserts on.
fn policed_entries() -> Vec<(PathBuf, String)> {
    let mut entries = Vec::new();
    for path in entry_asm_files() {
        if !reaches_kernel_image(&path) {
            println!(
                "census: {} is not assembled into the kernel image; reported, not asserted on",
                relative(&path)
            );
            continue;
        }
        let text = read_text(&path);
        for symbol in global_symbols(&text) {
            if is_hardware_entered(&symbol) {
                entries.push((path.clone(), symbol));
            } else {
                println!(
                    "census: {}:{} is declared in Rust as a parameter-taking callable, so it is \
                     entered by an ordinary call under the SysV ABI (DF clear on entry) and is \
                     exempt",
                    relative(&path),
                    symbol
                );
            }
        }
    }
    entries
}

#[test]
fn every_hand_written_entry_clears_df_before_its_first_call_or_string_op() {
    let entries = policed_entries();

    assert!(
        !entries.is_empty(),
        "the entry census derived 0 hand-written entry points, which would make this ratchet \
         vacuously true -- exactly the failure mode it exists to prevent"
    );

    let mut findings = Vec::new();
    for (path, symbol) in &entries {
        let text = read_text(path);
        let installed = if rust_takes_address(symbol) {
            "address-taken (installed in a descriptor table or MSR)"
        } else {
            "not address-taken in Rust; policed by default"
        };
        match missing_cld(&text, symbol) {
            None => println!("ok: {}:{} clears DF [{}]", relative(path), symbol, installed),
            Some(detail) => findings.push(format!(
                "{}:{} [{}] -- {}",
                relative(path),
                symbol,
                installed,
                detail
            )),
        }
    }

    assert!(
        findings.is_empty(),
        "{} of {} hand-written entry point(s) do not clear the direction flag before reaching a \
         call into Rust or a string operation. An x86-64 interrupt gate preserves DF, so such an \
         entry runs every rep-prefixed string operation backwards when it preempts a ring-3 \
         thread that legitimately holds DF=1 (#737):\n  {}",
        findings.len(),
        entries.len(),
        findings.join("\n  ")
    );
}

#[test]
fn the_asm_entry_census_is_not_vacuous() {
    let files = entry_asm_files();
    assert!(
        !files.is_empty(),
        "found 0 .asm files under kernel/src/interrupts/ or kernel/src/syscall/; the directory \
         census this ratchet is built on is reading nothing"
    );

    let assembled: Vec<&PathBuf> = files
        .iter()
        .filter(|path| reaches_kernel_image(path))
        .collect();
    assert!(
        !assembled.is_empty(),
        "none of the {} .asm file(s) found is reachable from kernel/build.rs or from a \
         global_asm/include_str/include in kernel/src; the reachability filter would drop \
         every entry and leave this ratchet asserting on nothing",
        files.len()
    );

    println!(
        "census: {} hand-written .asm file(s), {} assembled into the kernel image",
        files.len(),
        assembled.len()
    );

    let mut symbols = 0usize;
    for path in &assembled {
        symbols += global_symbols(&read_text(path)).len();
    }
    assert!(
        symbols > 0,
        "the assembled .asm files declare 0 global symbols, so step 2 of the census yields \
         nothing to police"
    );
    println!("census: {} global symbol(s) in the assembled files", symbols);
}

#[test]
fn llvm_generated_x86_interrupt_handlers_are_censused_and_exempt() {
    // These are exempt because LLVM's x86_intrcc prologue emits the cld
    // itself. The census exists so the exemption is countable rather than
    // asserted by prose, and so a future change that turns one of them into a
    // hand-written stub shows up as a drop here.
    let needle = "extern \"x86-interrupt\" fn";
    let mut total = 0usize;
    let mut files = 0usize;
    for source in files_with_extension(&repo_root().join("kernel/src"), "rs") {
        let count = read_text(&source).matches(needle).count();
        if count > 0 {
            println!("exempt census: {} declares {}", relative(&source), count);
            files += 1;
            total += count;
        }
    }
    assert!(
        total > 0,
        "found 0 compiler-generated x86-interrupt handlers in kernel/src; either that entry \
         family disappeared or this census stopped reading the tree"
    );
    println!(
        "exempt census: {} compiler-generated interrupt handler(s) across {} file(s)",
        total, files
    );
}

#[test]
fn the_analyzer_reddens_on_a_synthetic_entry_that_omits_cld() {
    // Anti-vacuity for the analyzer itself, independent of any tracked file:
    // the same missing_cld the ratchet uses must flag an entry that calls
    // into Rust with no cld, must flag one that runs a string operation with
    // no cld, and must clear one that has the cld first.
    let without = "global bad_entry\nbad_entry:\n    push rax\n    call rust_handler\n";
    let with = "global good_entry\ngood_entry:\n    push rax\n    cld\n    call rust_handler\n";
    let string_op = "global rep_entry\nrep_entry:\n    mov rcx, 8\n    rep movsq\n";

    assert!(
        missing_cld(without, "bad_entry").is_some(),
        "analyzer did not flag an entry that calls into Rust with no cld"
    );
    assert!(
        missing_cld(with, "good_entry").is_none(),
        "analyzer flagged an entry whose cld precedes its first call"
    );
    assert!(
        missing_cld(string_op, "rep_entry").is_some(),
        "analyzer did not flag an entry that runs a rep-prefixed string operation with no cld"
    );

    // And the comment stripper must not let a mentioned instruction count.
    let commented = "global commented_entry\ncommented_entry:\n    push rax ; call rust_handler\n";
    assert!(
        missing_cld(commented, "commented_entry").is_none(),
        "analyzer treated a call named inside a NASM comment as an instruction"
    );
}
