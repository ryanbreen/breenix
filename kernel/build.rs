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

            // Tell cargo to rerun if ANY userspace source changes
            // Watch the entire src directory so new files are automatically tracked
            let userspace_src = userspace_test_dir.join("src");
            println!("cargo:rerun-if-changed={}", userspace_test_dir.join("build.sh").display());
            println!("cargo:rerun-if-changed={}", userspace_test_dir.join("Cargo.toml").display());

            // Recursively watch all .rs files in userspace/tests/src/
            if userspace_src.exists() {
                watch_directory_recursive(&userspace_src);
            }

            // Also watch libbreenix library sources
            let libbreenix_src = repo_root.join("libs/libbreenix/src");
            if libbreenix_src.exists() {
                watch_directory_recursive(&libbreenix_src);
            }
        }
    } else {
        println!("cargo:warning=Userspace test directory not found at {:?}", userspace_test_dir);
    }
}

/// Recursively emit rerun-if-changed for all .rs files in a directory
fn watch_directory_recursive(dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                watch_directory_recursive(&path);
            } else if path.extension().map_or(false, |ext| ext == "rs") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}