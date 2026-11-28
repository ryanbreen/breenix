//! The ARM64 kernel heap and kernel-stack pool come from the memory map, not
//! from the platform (audit locals, DEV-29(4), DEV-27(2)).
//!
//! `arm64_boot_contract::phys_memory::place_kernel_pools` is checked against
//! synthetic memory maps by that crate's own tests. These checks pin that the
//! kernel uses it: no platform test chooses the heap size, and no fixed
//! physical address names the heap or the kernel-stack pool.

use std::fs;
use std::path::PathBuf;

fn repo_text(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(relative)).unwrap_or_else(|_| panic!("read {relative}"))
}

fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| line.find("//").map_or(line, |cut| &line[..cut]))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_heap_and_stack_pools_are_not_chosen_by_platform() {
    for file in ["kernel/src/memory/heap.rs", "kernel/src/memory/kernel_stack.rs"] {
        let code = code_only(&repo_text(file));
        assert!(!code.contains("is_qemu"), "{file} chooses its layout by platform");
        assert!(!code.contains("QEMU_HEAP_SIZE"), "{file} keeps a QEMU-only heap size");
    }
    let stacks = code_only(&repo_text("kernel/src/memory/kernel_stack.rs"));
    for fixed in ["0x5820_0000", "0x5A20_0000"] {
        assert!(!stacks.contains(fixed), "the kernel-stack pool is pinned at {fixed}");
    }
    let heap = code_only(&repo_text("kernel/src/memory/heap.rs"));
    assert!(!heap.contains("0x5020_0000"), "the ARM64 heap is pinned at 0x5020_0000");
}

#[test]
fn both_frame_pool_paths_place_the_kernel_pools() {
    let config = code_only(&repo_text("kernel/src/platform_config.rs"));
    assert!(
        config.matches("place_kernel_pools(").count() >= 1,
        "the frame pool must carve the heap and stacks out of the memory map"
    );
    let placed = config.find("fn with_kernel_pools").expect("one placement helper");
    let uses = config.matches("with_kernel_pools(").count();
    assert!(placed > 0 && uses >= 3, "the device-tree and UEFI paths must both place the pools");
    assert!(!config.contains("QEMU_HEAP_SIZE"), "the QEMU path keeps its own heap size");
}
