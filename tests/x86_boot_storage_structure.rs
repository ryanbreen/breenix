//! Structure check for E2.4x (ruling DEV-48): x86 `kernel_main` reaches no
//! storage read after `start_application_processors`.
//!
//! From that call on the APs dispatch, so the boot continuation is CPU 0's
//! idle thread running beside a scheduler that dispatches. A storage read from
//! there waits as idle: Completion's idle branch, AHCI admission's `CannotPark`
//! against a port a `breenfs-commit` worker owns (DEV-24), or the mount lock's
//! spin. E2.4 removes those idle waits (design §6, I8), so the storage reads
//! the continuation made move to the `boot_storage` kernel thread, which is
//! pinned to CPU 0 and may sleep.
//!
//! The check reads `kernel/src/main.rs` with comments and string literals
//! blanked. It takes the continuation, the body of
//! `kernel_main_on_kernel_stack_checked` after its
//! `start_application_processors()` call, and adds the bodies of the
//! `main.rs` functions that text calls, transitively. A function handed to a
//! kernel thread is named without a call (`kthread_run_on_cpu(f, ..)`), so it
//! is not part of the continuation. That text may call 0 of the storage entry
//! points.
//!
//! Scope: the entry points are named calls. A storage read hidden inside a
//! library function the continuation calls is outside what a source scan can
//! see; the list names 17 storage entry points, 10 of which the continuation
//! called on 5aa428fc.
//!
//! The PR #3283 review added three cases, at the end of the file: the thread
//! reads only at a `park::Braked` level; idle reaches no storage through the
//! x86 driver self-tests or the serial command handlers, two paths the call
//! scan above does not follow; and the BTRT loading bracket and the
//! completion reporter's start enclose the test processes the thread creates.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

fn main_source() -> String {
    source("kernel/src/main.rs")
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

/// The x86 boot continuation after AP start, with the main.rs functions it
/// reaches by a call.
fn continuation(code: &str) -> (String, BTreeSet<String>) {
    let entry = code
        .find("fn kernel_main_on_kernel_stack_checked(")
        .expect("the x86 continuation entry exists");
    let (_, end) = braced_body(code, entry);
    let ap_start = entry
        + code[entry..end]
            .find("start_application_processors()")
            .expect("the continuation starts the APs");
    reach(&functions(code), code[ap_start..end].to_string())
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

/// The storage entry points the x86 continuation called before E2.4x, and the
/// block-layer reads beneath them. `test_exec::` is the boot's test
/// registration module: each of its cases loads its binary from the test disk.
const STORAGE_CALLS: &[&str] = &[
    "install_requested_root",
    "mount_root",
    "mount_home",
    "bind_home_if_unmounted",
    "fence_seal",
    "run_fs_fault_leg",
    "run_ext2_lock_race_leg",
    "fence_fault::run",
    "read_init_from_ext2",
    "read_x86_production_init",
    "read_default_x86_init",
    "get_test_binary",
    "get_test_binary_static",
    "load_test_binary_from_disk",
    "read_block",
    "read_blocks",
    "load_image",
];

#[test]
fn x86_kernel_main_reaches_no_storage_read_after_ap_start() {
    let code = code_only(&main_source());
    let (text, reached) = continuation(&code);
    let mut findings: Vec<String> = STORAGE_CALLS
        .iter()
        .filter(|name| calls_or_invokes(&text, name))
        .map(|name| format!("calls {name}()"))
        .collect();
    if text.contains("test_exec::") {
        findings.push("calls into test_exec:: (test-binary registration)".to_string());
    }
    assert!(
        findings.is_empty(),
        "x86 kernel_main reads storage on the idle continuation after \
         start_application_processors (DEV-48). Through main.rs functions {:?} it:\n  {}",
        reached,
        findings.join("\n  ")
    );
}

/// The moved reads still happen, in the CPU-0 kernel thread: otherwise the
/// check above would pass on a tree that stopped mounting at all.
#[test]
fn the_boot_storage_thread_does_the_storage_work() {
    let code = code_only(&main_source());
    let defined = functions(&code);
    assert!(
        defined.contains_key("run_x86_boot_storage"),
        "no run_x86_boot_storage kernel-thread entry in main.rs"
    );
    let (continuation_text, _) = continuation(&code);
    let spawn = continuation_text
        .find("kthread_run_on_cpu(")
        .map(|at| &continuation_text[at..])
        .filter(|rest| rest.trim_start_matches("kthread_run_on_cpu(").trim_start().starts_with("run_x86_boot_storage"));
    assert!(
        spawn.is_some(),
        "the continuation does not hand run_x86_boot_storage to a kernel thread first"
    );
    // The thread's work: what its entry calls, and what it hands to `braked(`.
    let mut text = defined["run_x86_boot_storage"].join("\n");
    for name in handed_to_braked(&text) {
        if let Some(bodies) = defined.get(&name) {
            text.push('\n');
            text.push_str(&bodies.join("\n"));
        }
    }
    let (text, _) = reach(&defined, text);
    for name in [
        "install_requested_root",
        "mount_root",
        "mount_home",
        "bind_home_if_unmounted",
        "fence_seal",
        "read_x86_production_init",
        "get_test_binary",
    ] {
        assert!(calls(&text, name), "the boot storage thread no longer calls {name}()");
    }
    assert!(text.contains("test_exec::"), "the boot storage thread no longer registers the test programs");
}

#[test]
fn the_scan_sees_a_read_reached_through_a_helper() {
    let code = code_only(
        r#"
        fn kernel_main_on_kernel_stack_checked() -> ! {
            early_read();
            kernel::arch_impl::x86_64::ap_start::start_application_processors();
            helper();
            task::kthread::kthread_run_on_cpu(run_x86_boot_storage, "boot_storage", 0);
            // mount_root(); in a comment is not a call
            log::info!("mount_home() in a string is not a call");
            loop {}
        }
        fn early_read() { kernel::fs::vfs::mount_home(); }
        fn helper() { deeper(); }
        fn deeper() { kernel::fs::vfs::mount_root(); }
        fn run_x86_boot_storage() { kernel::block::fence_seal(); }
        "#,
    );
    let (text, reached) = continuation(&code);
    assert!(calls(&text, "mount_root"), "a read two calls deep is found");
    assert!(!calls(&text, "mount_home"), "a read before AP start, or in a comment or string, is not");
    assert!(!calls(&text, "fence_seal"), "a function handed to a kernel thread is not the continuation");
    assert_eq!(reached, BTreeSet::from(["deeper".to_string(), "helper".to_string()]));
}

// ------------------------------------------------- the review's cases (#3283)

/// The main.rs functions `text` names as the whole argument of `braked(`: the
/// boot_storage thread's work that runs at a `park::Braked` level.
fn handed_to_braked(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    text.match_indices("braked(")
        .filter(|(at, _)| *at == 0 || !is_ident(bytes[at - 1]))
        .filter_map(|(at, matched)| {
            let rest = text[at + matched.len()..].trim_start();
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            let after = rest[name.len()..].trim_start();
            (!name.is_empty() && after.starts_with(')')).then_some(name)
        })
        .collect()
}

/// The text inside the parentheses of the first `name(` call in `code`.
fn call_arguments<'a>(code: &'a str, name: &str) -> &'a str {
    let at = code.find(&format!("{name}(")).unwrap_or_else(|| panic!("no {name}( call"));
    let open = at + name.len();
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced {name}( call");
}

/// The bodies of `fn name(` in `raw` whose attributes name x86-64.
fn x86_bodies(raw: &str, name: &str) -> Vec<String> {
    let code = code_only(raw);
    code.match_indices(&format!("fn {name}("))
        .filter(|(at, _)| {
            // The item's attribute lines, read from the raw source: blanking
            // empties the `"x86_64"` literal. Offsets match.
            let item = raw[..*at].rfind("\n\n").map_or(0, |p| p + 2);
            raw[item..*at].contains("target_arch = \"x86_64\"")
        })
        .map(|(at, _)| {
            let (open, close) = braced_body(&code, at);
            code[open..close].to_string()
        })
        .collect()
}

/// Review Critical 1 and 2: the thread's storage work holds a `park::Braked`
/// level, as `breenfs-commit` and ARM64's `test-loader` do (design §3.2). At
/// baseline 0 the timer can preempt it while it holds AHCI's `PORT_IO_LOCK`,
/// a spin mutex, and a braked `breenfs-commit` dispatched on CPU 0 then spins
/// on that lock with the pinned holder unable to run. Its block waits also do
/// not sleep at baseline 0 (`completion::wait_sleeps` is idle or exactly one
/// braked level), so a VirtIO read past the 30 s bootstrap cutoff is abandoned
/// and its device wedged. So: the entry reaches no storage call except
/// through `braked(f)`, `braked` takes the level, and the functions handed to it
/// take no second one (Completion's sleep releases exactly one level).
#[test]
fn the_boot_storage_thread_reads_at_a_braked_level() {
    let code = code_only(&main_source());
    let defined = functions(&code);
    let entry = defined["run_x86_boot_storage"].join("\n");
    let (unbraked, through) = reach(&defined, entry.clone());
    // The lock-race driver makes no read itself: it waits for kernel threads that
    // take their own braked level (fs/ext2/lock_race.rs).
    let mut findings: Vec<String> = STORAGE_CALLS
        .iter()
        .filter(|name| **name != "run_ext2_lock_race_leg" && calls_or_invokes(&unbraked, name))
        .map(|name| format!("calls {name}()"))
        .collect();
    if unbraked.contains("test_exec::") {
        findings.push("calls into test_exec:: (test-binary registration)".to_string());
    }
    assert!(
        findings.is_empty(),
        "run_x86_boot_storage reads storage at baseline 0, outside braked(..). Through {:?} it:\n  {}",
        through,
        findings.join("\n  ")
    );
    let helper = defined.get("braked").map(|bodies| bodies.join("\n")).unwrap_or_default();
    assert!(helper.contains("Braked::new()"), "main.rs has no braked(..) helper taking a park::Braked level");
    let handed = handed_to_braked(&entry);
    assert!(
        handed.iter().any(|name| name == "mount_x86_boot_storage")
            && handed.iter().any(|name| name == "run_x86_boot_programs"),
        "the mounts and the program phase are not handed to braked(..): {handed:?}"
    );
    let work: Vec<String> = handed.iter().flat_map(|name| defined[name].clone()).collect();
    let (work, _) = reach(&defined, work.join("\n"));
    assert!(
        !work.contains("Braked::new()") && !calls(&work, "without_braked"),
        "the braked work changes its level again; a second level makes its block waits spin"
    );
}

/// Review Major 1: idle reaches storage through two paths a main.rs call scan
/// does not follow. The x86 driver self-tests (`drivers/mod.rs`) issue AHCI
/// IDENTIFY and the VirtIO and NVMe sector-0 reads, and the serial command
/// handlers that `register_handlers` installs load test programs from disk
/// when the console executor runs them. The self-tests' storage half runs on
/// the boot_storage thread, and each storage command on a braked kernel
/// thread of its own.
#[test]
fn idle_reaches_no_storage_through_self_tests_or_serial_commands() {
    let drivers = source("kernel/src/drivers/mod.rs");
    let post_init = x86_bodies(&drivers, "run_post_init_self_tests");
    assert_eq!(post_init.len(), 1, "one x86 run_post_init_self_tests in drivers/mod.rs");
    for name in ["ahci::init", "test_read"] {
        assert!(
            !calls_or_invokes(&post_init[0], name),
            "the x86 post-init self-tests, run on idle after AP start, call {name}()"
        );
    }
    let storage = x86_bodies(&drivers, "run_storage_self_tests");
    assert!(
        storage.len() == 1 && calls(&storage[0], "ahci::init") && calls_or_invokes(&storage[0], "test_read"),
        "drivers/mod.rs has no x86 run_storage_self_tests doing the AHCI init and the test reads"
    );
    let code = code_only(&main_source());
    let defined = functions(&code);
    let mount = defined.get("mount_x86_boot_storage").map(|bodies| bodies.join("\n")).unwrap_or_default();
    assert!(calls(&mount, "run_storage_self_tests"), "the boot_storage thread does not run the storage self-tests");

    let (handlers, _) = reach(&defined, call_arguments(&code, "register_handlers").to_string());
    let loaders = ["test_multiple_processes", "test_fork_debug", "test_userspace_fork", "test_exec_real_userspace"];
    let findings: Vec<&str> = STORAGE_CALLS
        .iter()
        .chain(loaders.iter())
        .copied()
        .filter(|name| calls_or_invokes(&handlers, name))
        .collect();
    assert!(findings.is_empty(), "a serial command handler reads storage on the executor's context: {findings:?}");
    let spawn = defined.get("serial_storage_command").map(|bodies| bodies.join("\n")).unwrap_or_default();
    assert!(
        spawn.contains("kthread_run(") && spawn.contains("Braked::new()"),
        "serial_storage_command does not run its command on a braked kernel thread"
    );
}

/// Review Major 2 and 3: tests run and exit while the thread is still
/// creating later ones. BTRT finalizes once its registered PIDs have exited
/// unless a loading bracket is open, and the completion reporter reports once
/// no user thread is live. So `btrt::begin_loading()` comes before the first
/// test process is created, and `btrt::end_loading()` and the reporter's
/// start come after the last one, the late `hello_time` included.
#[test]
fn test_registration_is_bracketed_until_the_last_test_process() {
    let code = code_only(&main_source());
    let defined = functions(&code);
    let body = defined["run_x86_boot_programs"].join("\n");
    let creates: Vec<usize> = body
        .match_indices("create_user_process(")
        .chain(body.match_indices("test_exec::"))
        .map(|(at, _)| at)
        .collect();
    let (first, last) = (*creates.iter().min().expect("test processes"), *creates.iter().max().expect("test processes"));
    let begin = body.find("btrt::begin_loading()");
    assert!(begin.is_some_and(|at| at < first), "no btrt::begin_loading() before the first test process");
    let end = body.rfind("btrt::end_loading()");
    assert!(end.is_some_and(|at| at > last), "no btrt::end_loading() after the last test process");
    let reporter = body.find("userspace_completion::start()");
    assert!(
        reporter.is_some_and(|at| at > last),
        "the completion reporter starts before the last test process is created"
    );
}
