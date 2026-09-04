//! #791: the kernel-thread dispatch path must not take a lock that ordinary
//! thread context holds with interrupts enabled.
//!
//! `interrupts::context_switch::setup_kernel_thread_return` runs inside the
//! timer interrupt with IF=0 and calls
//! `memory::process_memory::switch_to_kernel_page_table`, which reads the
//! master kernel PML4. While that read went through a `spin::Mutex`, a timer
//! preemption of `map_kernel_page`/`unmap_kernel_page` -- which held the same
//! mutex across a `log::trace!` because of the pre-2024 `if let` scrutinee
//! temporary lifetime -- left the lock held, and the dispatch performed by that
//! very interrupt spun on it forever with interrupts disabled. The two GDB
//! specimens are in
//! docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md.
//!
//! These assertions are census-anchored rather than line-pinned: they name the
//! accessor and its readers by shape, so a rename is still covered and a new
//! locking reader fails loudly.

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

/// The body of the first `fn <name>(` in `source`, up to the closing brace that
/// sits in the same column as the line the signature starts on. The 4 functions
/// this test reads -- `master_kernel_pml4`, `map_kernel_page`,
/// `unmap_kernel_page` and `setup_kernel_thread_return` -- are each written at
/// one indentation level, which is what makes that terminator exact; a function
/// that stops matching panics here rather than returning a short body.
/// claim-lint:ok: 4 of 4 callers below name their function literally.
fn function_body(source: &str, name: &str) -> String {
    let needle = format!("fn {name}(");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no `{needle}` in the source under test"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let terminator = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == terminator)
        .unwrap_or_else(|| panic!("no terminator for `{needle}`"))
        + start;
    lines[start..=end].join("\n")
}

#[test]
fn master_kernel_pml4_is_read_without_a_lock() {
    let kernel_page_table = read("kernel/src/memory/kernel_page_table.rs");

    let accessor = function_body(&kernel_page_table, "master_kernel_pml4");
    assert!(
        !accessor.contains(".lock()"),
        "master_kernel_pml4() is read from interrupt context with IF=0; it must not take a lock:\n{accessor}"
    );
    assert!(
        accessor.contains("MASTER_KERNEL_PML4_PHYS.load"),
        "master_kernel_pml4() must read the lock-free cell:\n{accessor}"
    );

    for line in kernel_page_table.lines() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        assert!(
            !(code.contains("MASTER_KERNEL_PML4") && code.contains("Mutex<")),
            "the master kernel PML4 must not live behind a Mutex: {line}"
        );
    }

    let cell_lines: Vec<&str> = kernel_page_table
        .lines()
        .filter(|line| {
            let code = line.trim_start();
            !code.starts_with("//") && code.contains("MASTER_KERNEL_PML4_PHYS")
        })
        .collect();
    assert_eq!(
        cell_lines.len(),
        3,
        "expected exactly the declaration, the single store and the single load; got:\n{}",
        cell_lines.join("\n")
    );
    assert_eq!(
        cell_lines
            .iter()
            .filter(|line| line.contains(".store("))
            .count(),
        1,
        "the master PML4 cell is written exactly once"
    );
}

#[test]
fn the_kernel_page_table_readers_go_through_the_accessor() {
    let kernel_page_table = read("kernel/src/memory/kernel_page_table.rs");
    for reader in ["map_kernel_page", "unmap_kernel_page"] {
        let body = function_body(&kernel_page_table, reader);
        assert!(
            !body.contains("MASTER_KERNEL_PML4"),
            "{reader} must reach the master PML4 through master_kernel_pml4(), not by locking the cell itself"
        );
        assert!(
            body.contains("master_kernel_pml4()"),
            "{reader} must read the master PML4 through the lock-free accessor"
        );
    }
}

#[test]
fn switch_to_kernel_page_table_takes_no_lock() {
    let process_memory = read("kernel/src/memory/process_memory.rs");
    let x86_arm = process_memory
        .split("pub unsafe fn switch_to_kernel_page_table()")
        .nth(2)
        .expect("two switch_to_kernel_page_table arms");
    let body = x86_arm
        .split("\n}\n")
        .next()
        .expect("a closing brace for the x86 arm");
    assert!(
        body.contains("master_kernel_pml4()"),
        "the x86 arm reads the master PML4 through the accessor"
    );
    assert!(
        !body.contains(".lock()"),
        "switch_to_kernel_page_table runs with IF=0 in the dispatch path; it must take no lock:\n{body}"
    );
}

#[test]
fn setup_kernel_thread_return_allocates_nothing() {
    let context_switch = read("kernel/src/interrupts/context_switch.rs");
    let body = function_body(&context_switch, "setup_kernel_thread_return");
    for allocating in ["name.clone()", "to_string()", "format!"] {
        assert!(
            !body.contains(allocating),
            "setup_kernel_thread_return runs with IF=0 on every kernel-thread dispatch and must not allocate; found `{allocating}`"
        );
    }
    assert!(
        body.contains("thread.context.clone()"),
        "the register context is still what this function restores"
    );
}
