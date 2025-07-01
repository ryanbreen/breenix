use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
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
            "-o", &format!("{}/timer_handler.o", out_dir),
            "src/interrupts/timer_handler.asm"
        ])
        .status()
        .expect("Failed to run nasm");
    
    if !status.success() {
        panic!("Failed to assemble timer handler");
    }
    
    // Tell cargo to link the assembled object files
    println!("cargo:rustc-link-arg={}/syscall_entry.o", out_dir);
    println!("cargo:rustc-link-arg={}/timer_handler.o", out_dir);
    
    // Rerun if the assembly files change
    println!("cargo:rerun-if-changed=src/syscall/entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/timer_handler.asm");
    
    // Build userspace test program if it exists
    let userspace_test_dir = Path::new("../userspace/tests");
    if userspace_test_dir.exists() {
        // Check if build script exists
        let build_script = userspace_test_dir.join("build.sh");
        if build_script.exists() {
            println!("cargo:warning=Building userspace test program...");
            
            let status = Command::new("bash")
                .arg(build_script)
                .current_dir(userspace_test_dir)
                .status()
                .expect("Failed to run userspace build script");
            
            if !status.success() {
                println!("cargo:warning=Failed to build userspace test program");
            } else {
                // Tell cargo to rerun if userspace source changes
                println!("cargo:rerun-if-changed=../userspace/tests/hello_time.rs");
                println!("cargo:rerun-if-changed=../userspace/tests/build.sh");
            }
        }
    }
}