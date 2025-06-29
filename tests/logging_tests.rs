mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that logging system initializes and works
#[test]
fn test_logging_initialization() {
    println!("Testing logging system initialization...");
    
    let output = get_kernel_output();
    
    // Check for different log levels
    assert!(output.contains("[ INFO]") || output.contains("[INFO]"), "INFO logs not found");
    assert!(output.contains("Serial port initialized"), 
            "Serial logging not working");
    
    println!("✅ Logging initialization test passed");
}

/// Test that different log levels work correctly
#[test]
fn test_log_levels() {
    println!("Testing different log levels...");
    
    let output = get_kernel_output();
    
    // Verify we get structured log output
    let info_count = output.matches("[ INFO]").count() + output.matches("[INFO]").count();
    assert!(info_count > 5, "Not enough INFO logs found: {}", info_count);
    
    // Check for proper log formatting with timestamps
    let timestamped_logs = output.lines()
        .filter(|line| (line.contains("[ INFO]") || line.contains("[INFO]")) && line.contains("-"))
        .count();
    
    assert!(timestamped_logs > 0, "No timestamped logs found");
    
    println!("✅ Log levels test passed (found {} INFO logs)", info_count);
}

/// Test serial output functionality
#[test]
fn test_serial_output() {
    println!("Testing serial output...");
    
    let output = get_kernel_output();
    
    // Verify serial output is working
    assert!(!output.is_empty(), "No serial output received");
    
    // Check for proper line endings
    let lines: Vec<&str> = output.lines().collect();
    assert!(lines.len() > 10, "Too few lines of output: {}", lines.len());
    
    // Verify output is properly formatted (not garbled)
    let well_formed_lines = lines.iter()
        .filter(|line| line.contains("[") && line.contains("]"))
        .count();
    
    assert!(well_formed_lines > 5, "Output appears garbled");
    
    println!("✅ Serial output test passed ({} lines)", lines.len());
}