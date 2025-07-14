use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // Tell Cargo to rerun this build script if BREENIX_TEST changes
    println!("cargo:rerun-if-env-changed=BREENIX_TEST");
    
    // Get the output directory
    let out_dir = env::var("OUT_DIR").unwrap();
    
    // Assemble syscall entry code
    let status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &format!("{}/syscall_entry.o", out_dir),
            "src/syscall/entry.asm"
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
            "src/interrupts/timer_entry.asm"
        ])
        .status()
        .expect("Failed to run nasm");
    
    if !status.success() {
        panic!("Failed to assemble timer entry");
    }
    
    // Assemble fault stubs
    let status = Command::new("nasm")
        .args(&[
            "-f", "elf64",
            "-o", &format!("{}/fault_stubs.o", out_dir),
            "src/fault_stubs.asm"
        ])
        .status()
        .expect("Failed to run nasm");
    
    if !status.success() {
        panic!("Failed to assemble fault stubs");
    }
    
    // Tell cargo to link the assembled object files
    println!("cargo:rustc-link-arg={}/syscall_entry.o", out_dir);
    println!("cargo:rustc-link-arg={}/timer_entry.o", out_dir);
    println!("cargo:rustc-link-arg={}/fault_stubs.o", out_dir);
    
    // Rerun if the assembly files change
    println!("cargo:rerun-if-changed=src/syscall/entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/timer_entry.asm");
    println!("cargo:rerun-if-changed=src/fault_stubs.asm");
    
    // Build userspace test programs
    build_userspace_tests();
}

fn build_userspace_tests() {
    // Only build if testing feature is enabled
    if env::var("CARGO_FEATURE_TESTING").is_err() {
        return;
    }
    
    println!("cargo:warning=Building userspace tests for kernel embedding...");
    
    // Get paths
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir).parent().unwrap();
    let userspace_dir = workspace_root.join("userspace/tests");
    
    // Ensure userspace directory exists
    if !userspace_dir.exists() {
        println!("cargo:warning=Userspace tests directory not found");
        return;
    }
    
    // Tell cargo to rerun if any userspace test changes
    println!("cargo:rerun-if-changed={}", userspace_dir.display());
    
    // Build all userspace tests
    println!("cargo:warning=Building userspace tests in release mode...");
    let output = Command::new("cargo")
        .current_dir(&userspace_dir)
        .args(&["build", "--release", "--bins"])
        .output()
        .expect("Failed to execute cargo build for userspace tests");
        
    if !output.status.success() {
        println!("cargo:warning=Failed to build userspace tests: {}", 
                 String::from_utf8_lossy(&output.stderr));
        return;
    }
    
    // List of all test binaries
    let test_binaries = [
        "hello_world",
        "hello_time", 
        "counter",
        "spinner",
        "fork_test",
        "spawn_test",
        "simple_wait_test",
        "wait_many",
        "waitpid_specific",
        "wait_nohang_polling",
        "echld_error",
        "fork_basic",
        "fork_mem_independent",
        "fork_deep_stack",
        "fork_progress_test",
        "fork_spin_stress",
        "exec_target",
        "exec_basic",
        "fork_exec_chain",
    ];
    
    // Copy built binaries to .elf files
    let target_dir = userspace_dir.join("target/x86_64-breenix/release");
    for binary in &test_binaries {
        let src = target_dir.join(binary);
        let dst = userspace_dir.join(format!("{}.elf", binary));
        
        if src.exists() {
            match fs::copy(&src, &dst) {
                Ok(_) => println!("cargo:warning=Copied {}.elf", binary),
                Err(e) => println!("cargo:warning=Failed to copy {}: {}", binary, e),
            }
        } else {
            println!("cargo:warning=Binary {} not found", binary);
        }
    }
}