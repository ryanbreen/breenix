mod shared_qemu;
use shared_qemu::{get_kernel_output, extract_timestamps};

/// Test that timer initializes and ticks
#[test]
fn test_timer_initialization() {
    println!("Testing timer initialization...");
    
    let output = get_kernel_output();
    
    // Check for timer initialization
    assert!(output.contains("Timer initialized"), 
            "Timer initialization not found");
    
    println!("✅ Timer initialization test passed");
}

/// Test timer tick functionality
#[test]
fn test_timer_ticks() {
    println!("Testing timer tick functionality...");
    
    let output = get_kernel_output();
    
    // Extract timestamps using shared helper and count unique ones
    let timestamps = extract_timestamps(output);
    // Convert to strings for HashSet since f64 doesn't implement Hash/Eq
    let timestamp_strings: Vec<String> = timestamps.iter().map(|t| format!("{}", t)).collect();
    let unique_timestamps: std::collections::HashSet<_> = timestamp_strings.iter().collect();
    
    // We should see multiple different timestamps (at least 2 for timer advancement)
    assert!(unique_timestamps.len() >= 2, 
            "Timer doesn't appear to be advancing: {} unique timestamps", 
            unique_timestamps.len());
    
    println!("✅ Timer ticks test passed ({} unique timestamps)", unique_timestamps.len());
}

/// Test delay functionality
#[test]
fn test_delay_functionality() {
    println!("Testing delay functionality...");
    
    let output = get_kernel_output();
    
    // Check for delay test
    assert!(output.contains("Testing delay macro"), 
            "Delay test not found");
    assert!(output.contains("Time after delay"), 
            "Delay completion not found");
    
    println!("✅ Delay functionality test passed");
}

/// Test RTC (Real Time Clock) functionality
#[test]
fn test_rtc_functionality() {
    println!("Testing RTC functionality...");
    
    let output = get_kernel_output();
    
    // Check for RTC reading
    assert!(output.contains("Current Unix timestamp"), 
            "RTC timestamp not found");
    
    // Verify we get a reasonable timestamp (after year 2020)
    if let Some(timestamp_line) = output.lines().find(|line| line.contains("Current Unix timestamp")) {
        if let Some(timestamp_str) = timestamp_line.split(':').last() {
            if let Ok(timestamp) = timestamp_str.trim().parse::<u64>() {
                // Unix timestamp for Jan 1, 2020
                assert!(timestamp > 1577836800, "RTC timestamp too old: {}", timestamp);
                println!("  RTC timestamp: {}", timestamp);
            }
        }
    }
    
    println!("✅ RTC functionality test passed");
}