use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "stage_markers.rs"]
mod stage_markers;

fn main() {
    // Emit a unique build ID based on current timestamp (seconds + subsecond nanos).
    // Baked into the kernel boot banner so stale builds are immediately detectable.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let build_id = format!(
        "{:010x}{:04x}",
        ts.as_secs(),
        (ts.subsec_nanos() >> 16) & 0xFFFF
    );
    println!("cargo:rustc-env=BREENIX_BUILD_ID={}", build_id);
    // Rerun whenever ANY kernel source file changes, so BUILD_ID always
    // reflects the actual build and never lies about staleness.
    //
    // NOTE: emitting any `cargo:rerun-if-changed` disables cargo's default
    // "rerun if anything in the package changed" behavior, so without this
    // directive BUILD_ID only re-stamped on the few files explicitly listed
    // below (e.g. xhci.rs) -- a stale BUILD_ID silently misled a validation
    // run once. Cargo recursively watches the whole tree when given a
    // directory path, so this restores honest "any source change" coverage.
    println!("cargo:rerun-if-changed=src");

    // Get absolute paths from Cargo environment
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let kernel_dir = PathBuf::from(&manifest_dir);
    let target = env::var("TARGET").unwrap_or_default();
    let use_linux_xhci = env::var("CARGO_FEATURE_XHCI_LINUX_HARNESS").is_ok();

    // The boot-path stage markers, so the kernel's output paths can keep the
    // lines a stage depends on whole (`stage_lines`).
    let repo_root = kernel_dir.parent().expect("kernel dir should have parent");
    for source in stage_markers::sources(repo_root) {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let markers = stage_markers::boot_path_markers(repo_root)
        .unwrap_or_else(|error| panic!("boot-path stage markers: {error}"));
    fs::write(
        PathBuf::from(&out_dir).join("stage_markers.rs"),
        format!("&{markers:?}"),
    )
    .expect("write stage_markers.rs");

    if target.starts_with("aarch64") {
        let ribbon=stage_markers::arm64_ribbon(repo_root).expect("ARM64 ribbon stages");
        assert!(!ribbon.is_empty() && ribbon.len() <= 416, "ARM64 boot ribbon exceeds its 1248-pixel segment capacity");
        fs::write(PathBuf::from(&out_dir).join("boot_ribbon.rs"),format!("&{ribbon:?}")).expect("write ribbon stages");
    }

    // Only build x86_64 assembly for x86_64 targets
    if target.contains("x86_64") {
        // Assemble syscall entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/syscall_entry.o", out_dir),
                kernel_dir.join("src/syscall/entry.asm").to_str().unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble syscall entry");
        }

        // Assemble timer interrupt entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/timer_entry.o", out_dir),
                kernel_dir
                    .join("src/interrupts/timer_entry.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble timer entry");
        }

        // Assemble breakpoint exception entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/breakpoint_entry.o", out_dir),
                kernel_dir
                    .join("src/interrupts/breakpoint_entry.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble breakpoint entry");
        }

        // Assemble the application-processor startup trampoline
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/ap_trampoline.o", out_dir),
                kernel_dir
                    .join("src/arch_impl/x86_64/ap_trampoline.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble AP trampoline");
        }

        // Tell cargo to link the assembled object files
        println!("cargo:rustc-link-arg={}/syscall_entry.o", out_dir);
        println!("cargo:rustc-link-arg={}/ap_trampoline.o", out_dir);
        println!("cargo:rustc-link-arg={}/timer_entry.o", out_dir);
        println!("cargo:rustc-link-arg={}/breakpoint_entry.o", out_dir);
    }

    // Use our custom linker script for x86_64
    // Temporarily disabled to test with bootloader's default
    // println!("cargo:rustc-link-arg=-Tkernel/linker.ld");

    // AArch64 kernel linker script (moved from aarch64-breenix.json so the
    // target JSON can be shared between kernel and userspace std builds)
    if target.contains("aarch64") {
        println!("cargo:rustc-link-arg=-Tkernel/src/arch_impl/aarch64/linker.ld");
        println!("cargo:rustc-link-arg=--fix-cortex-a53-843419");
        println!("cargo:rustc-link-arg=--orphan-handling=error");
    }

    // Build the Linux C-based xHCI harness when the feature is enabled.
    if use_linux_xhci {
        let src = kernel_dir.join("src/drivers/usb/linux_xhci/linux_xhci.c");
        let obj = PathBuf::from(&out_dir).join("linux_xhci.o");

        let mut cmd = Command::new("clang");
        cmd.arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .arg("-ffreestanding")
            .arg("-fno-builtin")
            .arg("-fno-stack-protector")
            .arg("-fno-omit-frame-pointer")
            .arg("-fno-pic")
            .arg("-fno-common")
            .arg("-Werror");

        if target.contains("aarch64") {
            cmd.arg("--target=aarch64-unknown-none")
                .arg("-mgeneral-regs-only");
        } else if target.contains("x86_64") {
            cmd.arg("--target=x86_64-unknown-none");
        }

        let status = cmd.status().expect("Failed to run clang for linux_xhci.c");
        if !status.success() {
            panic!("Failed to compile linux_xhci.c");
        }

        println!("cargo:rustc-link-arg={}", obj.display());
        println!("cargo:rerun-if-changed=src/drivers/usb/linux_xhci/linux_xhci.c");
        println!("cargo:rerun-if-changed=src/drivers/usb/linux_xhci/linux_xhci.h");
    }

    // Rerun if the assembly files change
    println!("cargo:rerun-if-changed=src/syscall/entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/timer_entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/breakpoint_entry.asm");
    println!("cargo:rerun-if-changed=src/arch_impl/x86_64/ap_trampoline.asm");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/arch_impl/aarch64/linker.ld");

    // Userspace is built by the launchers and xtask before packing their disks.
    // Never invoke Cargo (including through build.sh) from a build script: the
    // parent may hold the package-cache mutation lock for the entire build.
}
