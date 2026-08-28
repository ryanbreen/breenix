//! Structural oracle for the syscall-return-register contract (#608).
//!
//! Every Breenix syscall trap (`int 0x80` on x86-64, `svc #0` on aarch64) is
//! answered by the kernel writing the ABI return register: RAX on x86-64
//! (`kernel/src/syscall/handler.rs` sets it, `entry.asm` pops it), X0 on
//! aarch64. An `asm!` block that reaches the trap must therefore tell the
//! compiler that register is written. Declaring it as `in("rax")` promises the
//! opposite - that the block leaves the register untouched - and LLVM then
//! hoists the syscall-number load out of any enclosing loop. From the second
//! iteration on, the trap executes with whatever the kernel last returned as
//! its syscall number. That is #608: `clonevm_exec_test`'s `spin_until_u32`
//! degenerated into an unbounded `sys_read(count = 0)` fixed point.
//!
//! The rules below are shape rules over the repository's own Rust sources.
//! They name no file and carry no allowlist, so a new offender is caught the
//! moment it is written rather than when someone remembers to extend a list.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Directories that are not this repository's own sources: build outputs, the
/// vendored Rust fork, and the agent worktrees under `.claude`.
const SKIPPED_DIRS: [&str; 3] = ["target", "rust-fork", "node_modules"];

/// The trap mnemonics that reach the Breenix syscall entry.
const TRAP_MNEMONICS: [&str; 2] = ["int 0x80", "svc #0"];

/// The ABI return register of each trap, as it appears in an operand.
const RETURN_REGISTER_OPERANDS: [&str; 2] = ["in(\"rax\")", "in(\"x0\")"];

#[derive(Debug, Clone)]
struct AsmBlock {
    path: String,
    line: usize,
    macro_name: String,
    text: String,
}

impl AsmBlock {
    fn reaches_the_trap(&self) -> bool {
        TRAP_MNEMONICS
            .iter()
            .any(|mnemonic| self.text.contains(mnemonic))
    }

    /// `out(...)` also matches `lateout(...)` and `inlateout(...)`.
    fn declares_an_output(&self) -> bool {
        self.text.contains("out(")
    }

    fn never_returns(&self) -> bool {
        self.text.contains("noreturn")
    }

    fn names_the_return_register_as_input_only(&self) -> bool {
        RETURN_REGISTER_OPERANDS
            .iter()
            .any(|operand| self.text.contains(operand))
    }

    /// `naked_asm!` and `global_asm!` take no operands at all; the contract
    /// below is only meaningful for operand-carrying `asm!`.
    fn takes_operands(&self) -> bool {
        self.macro_name == "asm"
    }

    fn site(&self) -> String {
        format!("{}:{}", self.path, self.line)
    }
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Walk one Rust source and return every `asm!`-family invocation in it,
/// skipping macro calls that appear inside comments or string literals.
fn asm_blocks_in(path: &str, source: &str) -> Vec<AsmBlock> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                let mut depth = 1usize;
                while index < bytes.len() && depth != 0 {
                    if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                        depth += 1;
                        index += 2;
                    } else if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                        depth -= 1;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
            }
            b'"' => index = skip_string(bytes, index),
            b'\'' => index = skip_char_literal(bytes, index),
            b'r' | b'b' if raw_string_open(bytes, index).is_some() => {
                index = raw_string_end(bytes, index);
            }
            b'!' if index >= 3
                && source.is_char_boundary(index - 3)
                && &source[index - 3..index] == "asm" =>
            {
                let mut start = index - 3;
                while start > 0 && is_ident_byte(bytes[start - 1]) {
                    start -= 1;
                }
                let macro_name = source[start..index].to_string();
                let mut cursor = index + 1;
                while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'(') {
                    index += 1;
                    continue;
                }
                let end = balanced_end(bytes, cursor);
                blocks.push(AsmBlock {
                    path: path.to_string(),
                    line: source[..start].matches('\n').count() + 1,
                    macro_name,
                    text: source[start..end].to_string(),
                });
                index = end;
            }
            _ => index += 1,
        }
    }

    blocks
}

/// The hash count of a raw string literal opening at `index`, if one does.
/// Recognises `r"`, `r#"`, `br"`, `br#"` and any hash depth.
fn raw_string_open(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_ident_byte(bytes[index - 1]) {
        return None;
    }
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let mut hashes = 0usize;
    while bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'"') {
        Some(hashes)
    } else {
        None
    }
}

/// Index just past the raw string literal that opens at `index`.
fn raw_string_end(bytes: &[u8], index: usize) -> usize {
    let hashes = match raw_string_open(bytes, index) {
        Some(hashes) => hashes,
        None => return index + 1,
    };
    let mut cursor = index;
    while bytes.get(cursor) != Some(&b'"') {
        cursor += 1;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && bytes
                .get(cursor + 1..cursor + 1 + hashes)
                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
        {
            return cursor + 1 + hashes;
        }
        cursor += 1;
    }
    cursor
}

/// Index just past a character literal, leaving lifetimes alone.
fn skip_char_literal(bytes: &[u8], open: usize) -> usize {
    if bytes.get(open + 1) == Some(&b'\\') {
        let mut cursor = open + 2;
        while cursor < bytes.len() && cursor <= open + 8 {
            if bytes[cursor] == b'\'' {
                return cursor + 1;
            }
            cursor += 1;
        }
        return open + 1;
    }
    if bytes.get(open + 2) == Some(&b'\'') {
        return open + 3;
    }
    open + 1
}

/// Index just past the string literal that starts at `open`.
fn skip_string(bytes: &[u8], open: usize) -> usize {
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    index
}

/// Index just past the `)` that closes the `(` at `open`, ignoring parentheses
/// inside string literals and comments.
fn balanced_end(bytes: &[u8], open: usize) -> usize {
    let mut index = open + 1;
    let mut depth = 1usize;
    while index < bytes.len() && depth != 0 {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index),
            b'\'' => index = skip_char_literal(bytes, index),
            b'r' | b'b' if raw_string_open(bytes, index).is_some() => {
                index = raw_string_end(bytes, index);
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    index
}

fn collect_rust_sources(directory: &Path, root: &Path, sources: &mut Vec<(String, String)>) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect_rust_sources(&path, root, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if let Ok(contents) = fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                sources.push((relative, contents));
            }
        }
    }
}

/// Every trap-carrying `asm!` in the repository, in path order.
fn trap_asm_census() -> Vec<AsmBlock> {
    let root = repo_root();
    let mut sources = Vec::new();
    collect_rust_sources(&root, &root, &mut sources);
    sources.sort();

    let mut blocks: Vec<AsmBlock> = sources
        .iter()
        .flat_map(|(path, contents)| asm_blocks_in(path, contents))
        .filter(AsmBlock::reaches_the_trap)
        .collect();
    blocks.sort_by(|left, right| (&left.path, left.line).cmp(&(&right.path, right.line)));
    blocks
}

/// Rule A - the #608 shape exactly: the block names the trap's return register
/// as an input, declares no output for it, and is not `noreturn`.
fn input_only_return_register_violations(blocks: &[AsmBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter(|block| block.takes_operands())
        .filter(|block| {
            block.names_the_return_register_as_input_only()
                && !block.declares_an_output()
                && !block.never_returns()
        })
        .map(AsmBlock::site)
        .collect()
}

/// Rule B - the superset the same defect class lives in: a block that reaches
/// the trap and returns must declare the write the kernel performs, whether or
/// not it names the register as an input. A block that declares no operand for
/// the return register at all makes the same false promise.
fn undeclared_return_register_violations(blocks: &[AsmBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter(|block| block.takes_operands())
        .filter(|block| !block.declares_an_output() && !block.never_returns())
        .map(AsmBlock::site)
        .collect()
}

#[test]
fn the_census_sees_the_repository_s_syscall_traps() {
    let blocks = trap_asm_census();
    // Anti-vacuity: the rules below are only meaningful if the walk actually
    // reaches the sources. The floor is a census shape, not a count to be
    // maintained; it fails loudly if the walk is ever broken or narrowed.
    assert!(
        blocks.len() >= 20,
        "trap asm! census collapsed to {} blocks - the source walk is broken",
        blocks.len()
    );
    assert!(
        blocks.iter().any(|block| block.path.starts_with("userspace")),
        "census reached no userspace source"
    );
    assert!(
        blocks.iter().any(|block| block.path.starts_with("libs")),
        "census reached no library source"
    );
    assert!(
        blocks.iter().any(|block| block.path.starts_with("kernel")),
        "census reached no kernel source"
    );
}

#[test]
fn no_trap_names_its_return_register_as_input_only() {
    let violations = input_only_return_register_violations(&trap_asm_census());
    assert!(
        violations.is_empty(),
        "#608 shape: these asm! blocks promise the kernel leaves the syscall \
         return register untouched, so the syscall number is hoisted out of any \
         enclosing loop: {violations:?}"
    );
}

#[test]
fn every_returning_trap_declares_the_return_register_write() {
    let violations = undeclared_return_register_violations(&trap_asm_census());
    assert!(
        violations.is_empty(),
        "these asm! blocks reach the syscall trap and return, but declare no \
         output operand for the register the kernel writes: {violations:?}"
    );
}

#[test]
fn deliberately_broken_variants_fail_the_rules() {
    let broken_input_only = r#"
        unsafe fn sys_yield() {
            core::arch::asm!("int 0x80", in("rax") 24u64, options(nostack));
        }
    "#;
    let broken_input_only_aarch64 = r#"
        unsafe fn sys_yield() {
            core::arch::asm!("svc #0", in("x8") 124u64, in("x0") 0u64, options(nostack));
        }
    "#;
    let broken_undeclared = r#"
        unsafe fn probe() {
            core::arch::asm!("mov rax, 4", "int 0x80", options(nostack));
        }
    "#;

    for (label, planted) in [
        ("x86 input-only rax", broken_input_only),
        ("aarch64 input-only x0", broken_input_only_aarch64),
    ] {
        let blocks: Vec<AsmBlock> = asm_blocks_in("planted.rs", planted)
            .into_iter()
            .filter(AsmBlock::reaches_the_trap)
            .collect();
        assert_eq!(blocks.len(), 1, "{label}: planted block was not parsed");
        assert!(
            !input_only_return_register_violations(&blocks).is_empty(),
            "{label}: rule A did not reject the planted defect"
        );
        assert!(
            !undeclared_return_register_violations(&blocks).is_empty(),
            "{label}: rule B did not reject the planted defect"
        );
    }

    let blocks: Vec<AsmBlock> = asm_blocks_in("planted.rs", broken_undeclared)
        .into_iter()
        .filter(AsmBlock::reaches_the_trap)
        .collect();
    assert_eq!(blocks.len(), 1, "planted undeclared block was not parsed");
    assert!(
        input_only_return_register_violations(&blocks).is_empty(),
        "rule A is meant to be the narrower of the two rules"
    );
    assert!(
        !undeclared_return_register_violations(&blocks).is_empty(),
        "rule B did not reject a trap that declares no output at all"
    );
}

#[test]
fn correct_shapes_are_admitted() {
    let correct = r#"
        pub unsafe fn syscall0(num: u64) -> u64 {
            let ret: u64;
            asm!("int 0x80", in("rax") num, lateout("rax") ret, options(nostack));
            ret
        }
        unsafe fn thread_exit(code: u64) -> ! {
            core::arch::asm!("int 0x80", "2:", "pause", "jmp 2b",
                in("rax") 60u64, in("rdi") code, options(noreturn));
        }
        pub extern "C" fn __restore_rt() -> ! {
            core::arch::naked_asm!("mov x8, 139", "svc #0", "brk #1")
        }
    "#;
    let blocks: Vec<AsmBlock> = asm_blocks_in("planted.rs", correct)
        .into_iter()
        .filter(AsmBlock::reaches_the_trap)
        .collect();
    assert_eq!(blocks.len(), 3, "correct shapes were not all parsed");
    assert!(
        input_only_return_register_violations(&blocks).is_empty(),
        "rule A rejected a correct shape"
    );
    assert!(
        undeclared_return_register_violations(&blocks).is_empty(),
        "rule B rejected a correct shape"
    );
    assert!(
        blocks.iter().any(|block| block.macro_name == "naked_asm"),
        "the naked_asm! exemption was not exercised"
    );
}

/// A commented-out offender must not be reported, and a live one next to it
/// must still be.
#[test]
fn comments_and_strings_do_not_forge_findings() {
    let source = r#"
        // core::arch::asm!("int 0x80", in("rax") 24u64, options(nostack));
        /* core::arch::asm!("svc #0", in("x0") 0u64, options(nostack)); */
        const DOC: &str = "core::arch::asm!(\"int 0x80\", in(\"rax\") 24u64)";
        unsafe fn live() {
            core::arch::asm!("int 0x80", in("rax") 24u64, options(nostack));
        }
    "#;
    let blocks: Vec<AsmBlock> = asm_blocks_in("planted.rs", source)
        .into_iter()
        .filter(AsmBlock::reaches_the_trap)
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "expected exactly the live block, got {blocks:?}"
    );
    assert_eq!(input_only_return_register_violations(&blocks).len(), 1);
}
