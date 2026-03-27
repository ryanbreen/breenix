use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    // Rerun whenever xhci.rs changes (we always `touch` it before building,
    // so the build ID is always fresh for each deploy cycle).
    println!("cargo:rerun-if-changed=src/drivers/usb/xhci.rs");

    // Get absolute paths from Cargo environment
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let kernel_dir = PathBuf::from(&manifest_dir);
    let target = env::var("TARGET").unwrap_or_default();
    let use_linux_xhci = env::var("CARGO_FEATURE_XHCI_LINUX_HARNESS").is_ok();

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

        // Tell cargo to link the assembled object files
        println!("cargo:rustc-link-arg={}/syscall_entry.o", out_dir);
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
            cmd.arg("--target=aarch64-unknown-none");
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
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/arch_impl/aarch64/linker.ld");

    // Build userspace test programs with libbreenix
    // Use absolute path derived from CARGO_MANIFEST_DIR (kernel/)
    // NOTE: Skipped for aarch64 targets - ARM64 userspace binaries are built
    // separately via `userspace/programs/build.sh --arch aarch64` and the xtask
    // arm64-boot-stages command handles this.
    let repo_root = kernel_dir.parent().expect("kernel dir should have parent");
    let userspace_test_dir = repo_root.join("userspace/programs");

    if userspace_test_dir.exists() && !target.contains("aarch64") {
        let build_script = userspace_test_dir.join("build.sh");
        if build_script.exists() {
            let output = Command::new("bash")
                .arg(&build_script)
                .current_dir(&userspace_test_dir)
                .output()
                .expect("Failed to run userspace build script");

            if !output.status.success() {
                panic!("Failed to build userspace test programs with libbreenix");
            }

            // Tell cargo to rerun if userspace sources change
            let userspace_tests = userspace_test_dir.to_str().unwrap();
            let libbreenix_dir = repo_root.join("libs/libbreenix/src");
            println!("cargo:rerun-if-changed={}/build.sh", userspace_tests);
            println!("cargo:rerun-if-changed={}/hello_world.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/hello_time.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/fork_test.rs", userspace_tests);
            println!(
                "cargo:rerun-if-changed={}/clock_gettime_test.rs",
                userspace_tests
            );
            println!(
                "cargo:rerun-if-changed={}/udp_socket_test.rs",
                userspace_tests
            );
            println!(
                "cargo:rerun-if-changed={}/unix_socket_test.rs",
                userspace_tests
            );
            println!(
                "cargo:rerun-if-changed={}/unix_named_socket_test.rs",
                userspace_tests
            );
            println!("cargo:rerun-if-changed={}/tty_test.rs", userspace_tests);
            println!(
                "cargo:rerun-if-changed={}/job_control_test.rs",
                userspace_tests
            );
            println!("cargo:rerun-if-changed={}/session_test.rs", userspace_tests);
            println!(
                "cargo:rerun-if-changed={}/job_table_test.rs",
                userspace_tests
            );
            println!("cargo:rerun-if-changed={}/http_test.rs", userspace_tests);
            println!(
                "cargo:rerun-if-changed={}/pipeline_test.rs",
                userspace_tests
            );
            println!(
                "cargo:rerun-if-changed={}/sigchld_job_test.rs",
                userspace_tests
            );
            println!("cargo:rerun-if-changed={}/cwd_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/demo.rs", userspace_tests);
            println!(
                "cargo:rerun-if-changed={}/lib.rs",
                libbreenix_dir.to_str().unwrap()
            );

            // Also watch std build files
            let std_tests_dir = repo_root.join("userspace/programs");
            println!(
                "cargo:rerun-if-changed={}",
                std_tests_dir.join("build.sh").display()
            );
            println!(
                "cargo:rerun-if-changed={}",
                std_tests_dir.join("Cargo.toml").display()
            );
            let libbreenix_libc_dir = repo_root.join("libs/libbreenix-libc/src");
            println!(
                "cargo:rerun-if-changed={}",
                libbreenix_libc_dir.join("lib.rs").display()
            );
        }
    } else if !userspace_test_dir.exists() {
        panic!(
            "Userspace programs directory not found at {:?}. Build userspace first: bash userspace/programs/build.sh",
            userspace_test_dir
        );
    }
    // For aarch64 targets, userspace is built externally via build.sh --arch aarch64
}
