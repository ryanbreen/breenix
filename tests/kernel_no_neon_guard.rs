//! Durable guard against re-arming issue #528.
//!
//! The aarch64 kernel MUST be compiled with the soft-float kernel target
//! `aarch64-breenix-kernel.json` (`-neon,-fp-armv8`, `abi: softfloat`). A
//! correctly-built kernel therefore contains **zero** vector/FP load or store
//! instructions in its `.text` sections.
//!
//! If the kernel is ever built with the userspace NEON hardfloat target
//! `aarch64-breenix.json` by mistake — as throwaway gate scripts did, silently
//! re-arming #528 (see the #470 PR-1c RCA) — `compiler-builtins`' memcpy/memset
//! and ordinary register spills begin using `q`/`d`/`s`/`v` registers on the
//! kernel stack before the FPU trap is configured, producing the #528 fault
//! class.
//!
//! This host-side test builds the aarch64 kernel with the correct soft-float
//! target and runs `scripts/check-kernel-no-neon.sh`, which objdumps the ELF
//! and asserts zero non-allowlisted FP/SIMD load/store instructions in kernel
//! code. It is wired into the standard `cargo test` invocation and mirrors the
//! guard that `docker/qemu/run-aarch64-full-test.sh` runs on every boot test.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Build the aarch64 kernel with the SOFT-FLOAT kernel target, then assert the
/// resulting ELF contains no FP/SIMD load/store instructions in kernel code.
#[test]
fn kernel_elf_has_no_neon_fp_stores() {
    let root = repo_root();

    // Build the kernel with the correct soft-float target. The toolchain is
    // pinned via rust-toolchain.toml, so plain `cargo build` is correct.
    let build = Command::new("cargo")
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--target",
            "aarch64-breenix-kernel.json",
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
            "-p",
            "kernel",
            "--bin",
            "kernel-aarch64",
        ])
        .output()
        .expect("failed to spawn cargo build for the aarch64 kernel");

    if !build.status.success() {
        panic!(
            "aarch64 kernel build failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr),
        );
    }

    let kernel_elf = root.join("target/aarch64-breenix-kernel/release/kernel-aarch64");
    assert!(
        kernel_elf.exists(),
        "expected kernel ELF at {} after build",
        kernel_elf.display()
    );

    let guard = root.join("scripts/check-kernel-no-neon.sh");
    let check = Command::new("bash")
        .current_dir(&root)
        .arg(&guard)
        .arg(&kernel_elf)
        .output()
        .expect("failed to spawn scripts/check-kernel-no-neon.sh");

    let stdout = String::from_utf8_lossy(&check.stdout);
    let stderr = String::from_utf8_lossy(&check.stderr);
    println!("{stdout}");
    if !stderr.is_empty() {
        eprintln!("{stderr}");
    }

    assert!(
        check.status.success(),
        "NEON/FP guard tripped: the kernel ELF contains FP/SIMD load/store \
         instructions in kernel code. The kernel was almost certainly built \
         with the NEON hardfloat target (aarch64-breenix.json) instead of the \
         soft-float kernel target (aarch64-breenix-kernel.json), re-arming \
         issue #528. See the guard output above."
    );
}
