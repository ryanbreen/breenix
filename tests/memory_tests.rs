mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that memory management initializes correctly
#[test]
fn test_memory_initialization() {
    println!("Testing memory initialization...");
    
    let output = get_kernel_output();
    
    // Check for memory initialization messages
    assert!(output.contains("kernel::memory: Initializing memory management") || 
            output.contains("Memory management initialized"), 
            "Memory management initialization not found");
    assert!(output.contains("Frame allocator initialized"), 
            "Frame allocator initialization not found");
    assert!(output.contains("Heap initialized"), 
            "Heap initialization not found");
    
    // Check for memory size detection
    assert!(output.contains("MiB of usable memory"), 
            "Memory size detection failed");
    
    println!("✅ Memory initialization test passed");
}

/// Test heap allocation functionality
#[test]
fn test_heap_allocation() {
    println!("Testing heap allocation...");
    
    let output = get_kernel_output();
    
    // Check for heap allocation test
    assert!(output.contains("Heap allocation test passed") || 
            output.contains("Heap test: created vector"), 
            "Heap allocation test did not pass");
    assert!(output.contains("created vector with 10 elements") || 
            output.contains("Heap test: created vector with 10 elements"), 
            "Vector allocation test failed");
    assert!(output.contains("sum of elements = 45") || 
            output.contains("Heap test: sum of elements = 45"), 
            "Vector computation test failed");
    
    println!("✅ Heap allocation test passed");
}

/// Test memory regions enumeration
#[test]
fn test_memory_regions() {
    println!("Testing memory regions enumeration...");
    
    let output = get_kernel_output();
    
    // Check that we found memory regions
    assert!(output.contains("regions") || output.contains("MiB of usable memory"), 
            "Memory regions enumeration failed");
    
    // Check for reasonable memory size (at least 32MB)
    let found_memory = output.lines()
        .find(|line| line.contains("MiB of usable memory"))
        .expect("Memory size not reported");
    
    let memory_size: u64 = found_memory
        .split_whitespace()
        .find_map(|word| word.parse().ok())
        .expect("Could not parse memory size");
    
    assert!(memory_size >= 32, "Insufficient memory detected: {} MiB", memory_size);
    
    println!("✅ Memory regions test passed (found {} MiB)", memory_size);
}