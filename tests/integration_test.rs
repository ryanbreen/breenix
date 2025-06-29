use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use std::thread;
use std::sync::{Arc, Mutex};

#[test]
fn test_kernel_boots() {
    println!("Starting QEMU with kernel...");
    
    let mut cmd = Command::new("cargo")
        .args(&["run", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi", "--"])
        .args(&["-serial", "stdio"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start QEMU");

    let stdout = cmd.stdout.take().expect("Failed to capture stdout");
    let stderr = cmd.stderr.take().expect("Failed to capture stderr");
    
    // Collect all output
    let output = Arc::new(Mutex::new(String::new()));
    let output_clone = output.clone();
    
    // Read stdout in a separate thread
    let stdout_thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(line) = line {
                println!("STDOUT: {}", line);
                output_clone.lock().unwrap().push_str(&line);
                output_clone.lock().unwrap().push('\n');
            }
        }
    });
    
    // Read stderr in a separate thread
    let stderr_thread = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(line) = line {
                println!("STDERR: {}", line);
            }
        }
    });
    
    // Wait for a bit to collect output
    thread::sleep(Duration::from_secs(8));
    
    // Kill QEMU
    let _ = cmd.kill();
    let _ = cmd.wait();
    
    // Wait for threads to finish
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    
    // Check collected output
    let collected = output.lock().unwrap();
    println!("\n=== Collected Output ===\n{}", collected);
    println!("=== End Output ===\n");
    
    // Assert expected outputs were found
    assert!(collected.contains("Serial port initialized"), "Serial port initialization not found in output");
    assert!(collected.contains("Initializing kernel systems"), "Kernel initialization not found");
    assert!(collected.contains("Timer:") && collected.contains("elapsed"), "Timer output not found");
}

#[test]
fn test_interrupts_work() {
    let mut cmd = Command::new("cargo")
        .args(&["run", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi", "--"])
        .args(&["-serial", "stdio"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start QEMU");

    let stdout = cmd.stdout.take().expect("Failed to capture stdout");
    let reader = BufReader::new(stdout);
    
    let mut found_breakpoint = false;
    let mut found_timer_tick = false;
    
    // Set up timeout
    let timeout = Duration::from_secs(10);
    let child_id = cmd.id();
    thread::spawn(move || {
        thread::sleep(timeout);
        unsafe {
            libc::kill(child_id as i32, libc::SIGTERM);
        }
    });
    
    for line in reader.lines() {
        if let Ok(line) = line {
            println!("SERIAL: {}", line);
            
            if line.contains("EXCEPTION: BREAKPOINT") {
                found_breakpoint = true;
            }
            if line.contains("Timer:") && line.contains("1s elapsed") {
                found_timer_tick = true;
            }
            
            if found_breakpoint && found_timer_tick {
                break;
            }
        }
    }
    
    // Clean up
    let _ = cmd.kill();
    let _ = cmd.wait();
    
    assert!(found_breakpoint, "Breakpoint exception not handled");
    assert!(found_timer_tick, "Timer interrupts not working");
}