mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test interrupt system initialization
#[test]
fn test_interrupt_initialization() {
    println!("Testing interrupt system initialization...");
    
    let output = get_kernel_output();
    
    // Check for interrupt system initialization
    assert!(output.contains("GDT initialized"), 
            "GDT initialization not found");
    assert!(output.contains("IDT loaded successfully"), 
            "IDT initialization not found");
    assert!(output.contains("PIC initialized"), 
            "PIC initialization not found");
    assert!(output.contains("Interrupts enabled"), 
            "Interrupts not enabled");
    
    println!("✅ Interrupt initialization test passed");
}

/// Test breakpoint interrupt handling
#[test]
fn test_breakpoint_interrupt() {
    println!("Testing breakpoint interrupt handling...");
    
    let output = get_kernel_output();
    
    // Check for breakpoint test
    assert!(output.contains("EXCEPTION: BREAKPOINT"), 
            "Breakpoint exception not handled");
    assert!(output.contains("Breakpoint test completed"), 
            "Breakpoint test did not complete");
    
    println!("✅ Breakpoint interrupt test passed");
}

/// Test keyboard interrupt handling
#[test]
fn test_keyboard_interrupt() {
    println!("Testing keyboard interrupt setup...");
    
    let output = get_kernel_output();
    
    // Check for keyboard initialization
    assert!(output.contains("Keyboard queue initialized"), 
            "Keyboard queue not initialized");
    assert!(output.contains("Keyboard ready! Type to see characters"), 
            "Keyboard prompt not shown");
    
    println!("✅ Keyboard interrupt setup test passed");
}

/// Test timer interrupt is working (by checking for advancing timestamps)
#[test]
fn test_timer_interrupt() {
    println!("Testing timer interrupt functionality...");
    
    let output = get_kernel_output();
    
    // Extract all timestamps from log lines using shared helper
    let timestamps = shared_qemu::extract_timestamps(output);
    
    // Verify timestamps are increasing (timer interrupts are working)
    let mut increasing = true;
    for window in timestamps.windows(2) {
        if window[1] < window[0] {
            increasing = false;
            break;
        }
    }
    
    assert!(increasing, "Timer interrupts not advancing time monotonically");
    assert!(timestamps.len() > 10, "Too few timer updates: {}", timestamps.len());
    
    // Calculate average tick rate
    if timestamps.len() > 1 {
        let duration = timestamps.last().unwrap() - timestamps.first().unwrap();
        let tick_rate = timestamps.len() as f64 / duration;
        println!("  Timer interrupt rate: ~{:.1} Hz", tick_rate);
    }
    
    println!("✅ Timer interrupt test passed");
}

