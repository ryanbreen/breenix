//! Optional CLI for manual kernel test runs
//! 
//! Usage: cargo run -p breenix-test-runner --bin runner -- divide_by_zero

use breenix_test_runner::{run_test, markers};
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 2 {
        eprintln!("Usage: {} <test_name>", args[0]);
        eprintln!("Available tests: divide_by_zero, invalid_opcode, page_fault, multiple_processes");
        std::process::exit(1);
    }
    
    let test_name = &args[1];
    
    println!("🚀 Running kernel test: {}", test_name);
    
    match run_test(test_name) {
        Ok(run) => {
            println!("✅ Test completed successfully");
            
            // Show relevant markers for common tests
            let stdout = run.stdout_str();
            match test_name.as_str() {
                "divide_by_zero" => {
                    if stdout.contains(markers::DIV0_OK) {
                        println!("✅ Found marker: {}", markers::DIV0_OK);
                    }
                }
                "invalid_opcode" => {
                    if stdout.contains(markers::UD_OK) {
                        println!("✅ Found marker: {}", markers::UD_OK);
                    }
                }
                "page_fault" => {
                    if stdout.contains(markers::PF_OK) {
                        println!("✅ Found marker: {}", markers::PF_OK);
                    }
                }
                "multiple_processes" => {
                    let hello_count = run.count_pattern("Hello from userspace!");
                    println!("✅ Found {} 'Hello from userspace!' messages", hello_count);
                }
                _ => {
                    println!("ℹ️  Output length: {} bytes", stdout.len());
                }
            }
        }
        Err(e) => {
            eprintln!("❌ Test failed: {}", e);
            std::process::exit(1);
        }
    }
}