//! Structure checks for E2.4, the block I/O caller rule
//! (docs/plans/e2-sleepable-contexts.md §6 and §8, "E2.4"; ruling DEV-24).
//!
//! Block I/O requires a sleepable context or the early-boot token. So:
//!
//! * AHCI's `CannotPark` refusal, its admission `yield` poll and the level
//!   arithmetic around the admission wait are gone from the kernel;
//! * ARM64 `kernel_main` reaches no storage read and no `spawn_as_current`
//!   once the scheduler dispatches. The first dispatch follows
//!   `timer_interrupt::init()`: CPU 0's timer runs from there, and the
//!   secondaries are released after it. PID 1 is loaded and started by the
//!   `init-loader` kernel thread, through the scheduler;
//! * the testing `test-loader` holds its CPU through a `park::Braked` guard,
//!   never a bare `preempt_disable()`.
//!
//! The scan reads `kernel/src/main_aarch64.rs` with comments and string
//! literals blanked, takes `kernel_main`'s body after its
//! `timer_interrupt::init()` call, and adds the bodies of the functions in
//! that file the text calls, transitively. A function handed to a kernel
//! thread is named without a call (`kthread_run(f, ..)`), so it is not part of
//! the continuation. A read hidden inside a library function the
//! continuation calls is outside what the scan sees; the list names the reads
//! `main_aarch64.rs` makes itself.
//!
//! Run: scripts/run-structure-tests.sh block_io_caller_structure

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

/// The source with comments and the contents of string and character
/// literals replaced by spaces, so offsets and brace structure are kept.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, at: usize| {
        if out[at] != b'\n' {
            out[at] = b' ';
        }
    };
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    blank(&mut out, i);
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        blank(&mut out, i);
                        blank(&mut out, i + 1);
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        blank(&mut out, i);
                        blank(&mut out, i + 1);
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        blank(&mut out, i);
                        i += 1;
                    }
                }
            }
            b'r' if matches!(bytes.get(i + 1), Some(b'"') | Some(b'#'))
                && (i == 0 || !is_ident(bytes[i - 1])) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while bytes.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if bytes.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                loop {
                    if j >= bytes.len() {
                        break;
                    }
                    if bytes[j] == b'"' && bytes[j + 1..].iter().take(hashes).filter(|&&b| b == b'#').count() == hashes {
                        j += 1 + hashes;
                        break;
                    }
                    blank(&mut out, j);
                    j += 1;
                }
                i = j;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        blank(&mut out, i);
                        i += 1;
                    }
                    blank(&mut out, i);
                    i += 1;
                }
                i += 1;
            }
            b'\'' => {
                // A character literal, not a lifetime: 'x', '\n', '\''.
                let literal_end = if bytes.get(i + 1) == Some(&b'\\') {
                    bytes[i + 2..].iter().position(|&b| b == b'\'').map(|p| i + 2 + p)
                } else if bytes.get(i + 2) == Some(&b'\'') {
                    Some(i + 2)
                } else {
                    None
                };
                match literal_end {
                    Some(end) => {
                        for at in i + 1..end {
                            blank(&mut out, at);
                        }
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("blanking keeps UTF-8 boundaries of ASCII syntax")
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The text between the `{` that opens at or after `from` and its match.
fn braced_body(code: &str, from: usize) -> (usize, usize) {
    let bytes = code.as_bytes();
    let open = from + code[from..].find('{').expect("a body follows");
    let mut depth = 0usize;
    for (offset, &byte) in bytes[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (open, open + offset + 1);
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced body at {from}");
}

/// The `fn name(` items defined in the file, each with its bodies (cfg variants
/// share a name).
fn functions(code: &str) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let bytes = code.as_bytes();
    for (at, _) in code.match_indices("fn ") {
        if at > 0 && is_ident(bytes[at - 1]) {
            continue;
        }
        let name: String = code[at + 3..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        let after = at + 3 + name.len();
        let rest = code[after..].trim_start();
        if !rest.starts_with('(') && !rest.starts_with('<') {
            continue;
        }
        // A declaration without a body (an extern item) ends at `;` first.
        let semi = code[after..].find(';').map(|p| after + p);
        let brace = code[after..].find('{').map(|p| after + p);
        match (brace, semi) {
            (Some(b), Some(s)) if s < b => continue,
            (None, _) => continue,
            _ => {}
        }
        let (open, close) = braced_body(code, after);
        found.entry(name).or_default().push(code[open..close].to_string());
    }
    found
}

/// Whether `text` calls `name(` as a free or path-qualified function, not as
/// a method and not as a definition.
fn calls(text: &str, name: &str) -> bool {
    calls_as(text, name, false)
}

/// [`calls`], counting a method call (`disk.read_blocks(`) too: for storage
/// entry points, where a method is as much a read as a function.
fn calls_or_invokes(text: &str, name: &str) -> bool {
    calls_as(text, name, true)
}

fn calls_as(text: &str, name: &str, methods: bool) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(name).any(|(at, _)| {
        let before_ok = at == 0 || (!is_ident(bytes[at - 1]) && (methods || bytes[at - 1] != b'.'));
        let defined = text[..at].trim_end().ends_with("fn");
        let after = text[at + name.len()..].trim_start();
        before_ok && !defined && after.starts_with('(')
    })
}

/// `text` with the bodies of the main.rs functions it calls appended,
/// transitively, and the names of those functions.
fn reach(defined: &BTreeMap<String, Vec<String>>, mut text: String) -> (String, BTreeSet<String>) {
    let mut reached = BTreeSet::new();
    loop {
        let next: Vec<String> = defined
            .keys()
            .filter(|name| !reached.contains(*name) && calls(&text, name))
            .cloned()
            .collect();
        if next.is_empty() {
            break;
        }
        for name in next {
            for body in &defined[&name] {
                text.push('\n');
                text.push_str(body);
            }
            reached.insert(name);
        }
    }
    (text, reached)
}

/// ARM64 `kernel_main` from the first dispatch on, with the functions of
/// `main_aarch64.rs` it reaches by a call.
fn arm64_continuation(code: &str) -> (String, BTreeSet<String>) {
    let entry = code.find("fn kernel_main(").expect("the ARM64 kernel_main exists");
    let (_, end) = braced_body(code, entry);
    let dispatch = entry
        + code[entry..end]
            .find("timer_interrupt::init()")
            .expect("kernel_main starts the timer");
    reach(&functions(code), code[dispatch..end].to_string())
}

/// The storage reads `main_aarch64.rs` makes, and the entry that bypasses the
/// scheduler to run PID 1 on the boot continuation.
const STORAGE_CALLS: &[&str] = &[
    "read_init_from_ext2",
    "run_userspace_from_disk",
    "load_image",
    "load_test_binaries_from_ext2",
    "mount_root",
    "mount_home",
    "bind_home_if_unmounted",
    "read_block",
    "read_blocks",
    "spawn_as_current",
    "launch_init_from_elf",
    "launch",
];

#[test]
fn arm64_kernel_main_reaches_no_storage_read_after_the_first_dispatch() {
    let code = code_only(&source("kernel/src/main_aarch64.rs"));
    let (text, reached) = arm64_continuation(&code);
    let findings: Vec<String> = STORAGE_CALLS
        .iter()
        .filter(|name| calls_or_invokes(&text, name))
        .map(|name| format!("calls {name}()"))
        .collect();
    assert!(
        findings.is_empty(),
        "ARM64 kernel_main reads storage or starts PID 1 on the boot continuation after the first \
         dispatch (design §6). Through main_aarch64.rs functions {:?} it:\n  {}",
        reached,
        findings.join("\n  ")
    );
}

/// The moved reads still happen, in the `init-loader` kernel thread, and the
/// continuation hands that thread its work: otherwise the check above would
/// pass on a tree that stopped launching init.
#[test]
fn the_init_loader_thread_loads_and_starts_pid1() {
    let code = code_only(&source("kernel/src/main_aarch64.rs"));
    let defined = functions(&code);
    let body = defined
        .get("run_init_loader")
        .map(|bodies| bodies.join("\n"))
        .expect("no run_init_loader kernel-thread entry in main_aarch64.rs");
    let (text, _) = reach(&defined, body);
    for read in ["read_init_from_ext2", "run_userspace_from_disk"] {
        assert!(calls(&text, read), "run_init_loader does not reach {read}()");
    }
    let (continuation, _) = arm64_continuation(&code);
    assert!(
        continuation.contains("run_init_loader") && !calls(&continuation, "run_init_loader"),
        "the continuation does not hand run_init_loader to a kernel thread"
    );
}

/// The testing loader keeps its CPU between binaries through `park::Braked`,
/// which raises its baseline with its depth, so its block waits are a
/// sleepable context. A bare `preempt_disable()` would be `LocksHeld(1)`.
#[test]
fn the_test_loader_holds_park_braked_and_no_bare_level() {
    let code = code_only(&source("kernel/src/main_aarch64.rs"));
    let defined = functions(&code);
    let body = defined.get("run_test_loader").map(|bodies| bodies.join("\n")).expect("run_test_loader exists");
    assert!(body.contains("park::Braked::new()"), "run_test_loader takes no park::Braked guard");
    for bare in ["preempt_disable()", "preempt_enable()"] {
        assert!(!body.contains(bare), "run_test_loader changes its level with a bare {bare}");
    }
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `CannotPark` is gone: a block read can no longer be refused for who is
/// asking and turned into EAGAIN. The kernel and the host tests that mirror
/// its errors name it 0 times.
#[test]
fn cannot_park_is_gone() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["kernel/src", "tools/breenfs-host-tests/src", "tools/vfs-host-tests/src", "tools/park-host-tests/src"] {
        rust_files(&root.join(dir), &mut files);
    }
    let found: Vec<String> = files
        .iter()
        .filter(|path| code_only(&fs::read_to_string(path).unwrap()).contains("CannotPark"))
        .map(|path| path.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert!(found.is_empty(), "CannotPark is still named in:\n  {}", found.join("\n  "));
}

/// AHCI admission neither polls with `yield` nor adjusts the preemption level
/// around its wait: a busy port parks a sleepable caller, and the park
/// releases and restores the caller's baseline itself.
#[test]
fn ahci_admission_has_no_yield_poll_and_no_level_arithmetic() {
    let code = code_only(&source("kernel/src/drivers/ahci/mod.rs"));
    let defined = functions(&code);
    assert!(!defined.contains_key("relax_port_io_wait"), "the admission yield helper is still defined");
    let body = defined.get("begin_port_io").map(|bodies| bodies.join("\n")).expect("begin_port_io exists");
    let findings: Vec<&str> = ["yield", "relax_port_io_wait", "preempt_disable", "preempt_enable", "Admission::Poll"]
        .into_iter()
        .filter(|needle| body.contains(needle))
        .collect();
    assert!(findings.is_empty(), "begin_port_io still contains {findings:?}");
}
