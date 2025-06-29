mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that async executor starts after kernel initialization
#[test]
fn test_async_executor_starts() {
    println!("Testing async executor startup...");
    
    let output = get_kernel_output();
    
    // Verify that kernel POST tests completed first
    assert!(output.contains("🎯 KERNEL_POST_TESTS_COMPLETE 🎯"), 
            "Kernel POST tests not completed");
    
    // Check for async executor startup
    assert!(output.contains("Starting async executor..."), 
            "Async executor not started");
    
    // Check that the executor message comes after POST completion
    let post_index = output.find("🎯 KERNEL_POST_TESTS_COMPLETE 🎯").unwrap();
    let executor_index = output.find("Starting async executor...").unwrap();
    
    assert!(executor_index > post_index, 
            "Async executor started before POST completion");
    
    println!("✓ Async executor startup test passed");
}

/// Test that keyboard task is spawned correctly
#[test]
fn test_keyboard_task_spawned() {
    println!("Testing keyboard task spawning...");
    
    let output = get_kernel_output();
    
    // Check for async executor startup
    assert!(output.contains("Starting async executor..."), 
            "Async executor not started");
    
    // Check for keyboard task
    assert!(output.contains("Keyboard ready! Type to see characters"), 
            "Keyboard task not spawned");
    
    println!("✓ Keyboard task spawning test passed");
}

/// Test the ordering of async executor components
#[test]
fn test_async_executor_ordering() {
    println!("Testing async executor component ordering...");
    
    let output = get_kernel_output();
    
    // Find all relevant lines
    let lines: Vec<&str> = output.lines().collect();
    
    let mut post_complete_found = false;
    let mut executor_started = false;
    let mut keyboard_ready = false;
    
    for line in lines {
        if line.contains("🎯 KERNEL_POST_TESTS_COMPLETE 🎯") {
            post_complete_found = true;
        } else if line.contains("Starting async executor...") {
            assert!(post_complete_found, 
                    "Async executor started before POST completion");
            executor_started = true;
        } else if line.contains("Keyboard ready! Type to see characters") {
            assert!(executor_started, 
                    "Keyboard task started before executor");
            keyboard_ready = true;
        }
    }
    
    assert!(post_complete_found, "POST completion marker not found");
    assert!(executor_started, "Async executor not started");
    assert!(keyboard_ready, "Keyboard task not ready");
    
    println!("✓ Async executor ordering test passed");
}