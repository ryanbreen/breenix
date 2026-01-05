use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Get absolute paths from Cargo environment
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let kernel_dir = PathBuf::from(&manifest_dir);

    // Assemble syscall entry code
    let status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &format!("{}/syscall_entry.o", out_dir),
            kernel_dir.join("src/syscall/entry.asm").to_str().unwrap()
        ])
        .status()
        .expect("Failed to run nasm");
    
    if !status.success() {
        panic!("Failed to assemble syscall entry");
    }
    
    // Assemble timer interrupt entry code
    let status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &format!("{}/timer_entry.o", out_dir),
            kernel_dir.join("src/interrupts/timer_entry.asm").to_str().unwrap()
        ])
        .status()
        .expect("Failed to run nasm");

    if !status.success() {
        panic!("Failed to assemble timer entry");
    }

    // Assemble breakpoint exception entry code
    let status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &format!("{}/breakpoint_entry.o", out_dir),
            kernel_dir.join("src/interrupts/breakpoint_entry.asm").to_str().unwrap()
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
    
    // Use our custom linker script
    // Temporarily disabled to test with bootloader's default
    // println!("cargo:rustc-link-arg=-Tkernel/linker.ld");
    
    // Rerun if the assembly files change
    println!("cargo:rerun-if-changed=src/syscall/entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/timer_entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/breakpoint_entry.asm");
    println!("cargo:rerun-if-changed=linker.ld");
    
    // Build userspace test programs with libbreenix
    // Use absolute path derived from CARGO_MANIFEST_DIR (kernel/)
    let repo_root = kernel_dir.parent().expect("kernel dir should have parent");
    let userspace_test_dir = repo_root.join("userspace/tests");

    if userspace_test_dir.exists() {
        let build_script = userspace_test_dir.join("build.sh");
        if build_script.exists() {
            println!("cargo:warning=");
            println!("cargo:warning=Building userspace binaries with libbreenix...");

            let output = Command::new("bash")
                .arg(&build_script)
                .current_dir(&userspace_test_dir)
                .output()
                .expect("Failed to run userspace build script");

            // Print the build output so user sees libbreenix compilation
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("cargo:warning={}", line);
            }

            if !output.status.success() {
                for line in String::from_utf8_lossy(&output.stderr).lines() {
                    println!("cargo:warning=STDERR: {}", line);
                }
                panic!("Failed to build userspace test programs with libbreenix");
            }

            // Tell cargo to rerun if userspace sources change
            let userspace_tests = userspace_test_dir.to_str().unwrap();
            let libbreenix_dir = repo_root.join("libs/libbreenix/src");
            println!("cargo:rerun-if-changed={}/build.sh", userspace_tests);
            println!("cargo:rerun-if-changed={}/hello_world.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/hello_time.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/fork_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/clock_gettime_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/udp_socket_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/tty_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/file_read_test.rs", userspace_tests);
            println!("cargo:rerun-if-changed={}/lib.rs", libbreenix_dir.to_str().unwrap());
            println!("cargo:rerun-if-changed={}/fs.rs", libbreenix_dir.to_str().unwrap());
        }
    } else {
        println!("cargo:warning=Userspace test directory not found at {:?}", userspace_test_dir);
    }

    // Build Rust std test programs (userspace/tests-std)
    // These require libbreenix-libc (libc.a) to be built first
    let libbreenix_libc_dir = repo_root.join("libs/libbreenix-libc");
    let userspace_std_dir = repo_root.join("userspace/tests-std");

    if libbreenix_libc_dir.exists() && userspace_std_dir.exists() {
        println!("cargo:warning=");
        println!("cargo:warning=Building Rust std test programs...");

        // Step 1: Build libbreenix-libc (produces libc.a)
        println!("cargo:warning=  Step 1: Building libbreenix-libc...");
        let target_json = repo_root.join("x86_64-breenix.json");
        let output = Command::new("cargo")
            .args(&[
                "build",
                "--release",
                "--target",
                target_json.to_str().unwrap(),
            ])
            .current_dir(&libbreenix_libc_dir)
            .output()
            .expect("Failed to build libbreenix-libc");

        if !output.status.success() {
            println!("cargo:warning=libbreenix-libc build failed:");
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                println!("cargo:warning=  {}", line);
            }
            panic!("Failed to build libbreenix-libc");
        }
        println!("cargo:warning=  libbreenix-libc built successfully");

        // Step 2: Build userspace/tests-std with -Z build-std
        // The .cargo/config.toml in tests-std already has:
        // - build-std = ["std", "panic_abort"]
        // - build-std-features = ["compiler-builtins-mem"]
        // - rustflags with library path to libbreenix-libc
        // So we just need to run cargo +nightly build --release
        //
        // IMPORTANT: We must clear CARGO_ and RUST_ env vars inherited from the
        // kernel build, as they can interfere with the tests-std build.
        println!("cargo:warning=  Step 2: Building tests-std with Rust std...");

        let output = Command::new("cargo")
            .args(&[
                "+nightly",
                "build",
                "--release",
            ])
            .current_dir(&userspace_std_dir)
            // Clear inherited cargo/rust environment to avoid conflicts
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET")
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_PKG_NAME")
            .env_remove("OUT_DIR")
            .output()
            .expect("Failed to build tests-std");

        if !output.status.success() {
            println!("cargo:warning=tests-std build failed:");
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                println!("cargo:warning=  {}", line);
            }
            panic!("Failed to build tests-std");
        }
        println!("cargo:warning=  tests-std built successfully (hello_std_real)");

        // Tell cargo to rerun if std test sources change
        let libbreenix_libc_src = libbreenix_libc_dir.join("src/lib.rs");
        let hello_std_real_src = userspace_std_dir.join("src/hello_std_real.rs");
        println!("cargo:rerun-if-changed={}", libbreenix_libc_src.to_str().unwrap());
        println!("cargo:rerun-if-changed={}", hello_std_real_src.to_str().unwrap());
        println!("cargo:rerun-if-changed={}", userspace_std_dir.join("Cargo.toml").to_str().unwrap());
    }
}