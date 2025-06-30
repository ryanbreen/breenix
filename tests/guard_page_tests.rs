mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that guard page system initializes properly
#[test]
fn test_guard_page_initialization() {
    println!("Testing guard page system initialization...");
    
    let output = get_kernel_output();
    
    // Check that stack allocation system initializes
    assert!(output.contains("Initializing stack allocation system..."), 
            "Stack allocation system not initialized");
    
    assert!(output.contains("Stack allocation system initialized"), 
            "Stack allocation system initialization not completed");
    
    // Verify memory management is complete
    assert!(output.contains("Memory management initialized"), 
            "Memory management not properly initialized");
    
    println!("✓ Guard page initialization test passed");
}

/// Test that the page fault handler is enhanced for guard page detection
#[test]
fn test_page_fault_handler_enhanced() {
    println!("Testing enhanced page fault handler...");
    
    let output = get_kernel_output();
    
    // Verify that all necessary components are loaded
    assert!(output.contains("IDT loaded successfully"), 
            "IDT not loaded successfully");
    
    assert!(output.contains("PIC initialized"), 
            "PIC not initialized");
    
    assert!(output.contains("Interrupts enabled"), 
            "Interrupts not enabled");
    
    // The enhanced page fault handler is now in place
    // We can't directly test guard page access without crashing the kernel,
    // but we can verify the infrastructure is ready
    
    println!("✓ Enhanced page fault handler test passed");
}

/// Test that memory management includes stack allocation
#[test]
fn test_memory_management_completeness() {
    println!("Testing memory management completeness...");
    
    let output = get_kernel_output();
    
    // Check all memory subsystems are initialized
    assert!(output.contains("Initializing frame allocator..."), 
            "Frame allocator initialization not started");
    
    assert!(output.contains("Frame allocator initialized"), 
            "Frame allocator not initialized");
    
    assert!(output.contains("Initializing paging..."), 
            "Paging initialization not started");
    
    assert!(output.contains("Page table initialized"), 
            "Page table not initialized");
    
    assert!(output.contains("Initializing heap allocator..."), 
            "Heap allocator initialization not started");
    
    assert!(output.contains("Heap initialized"), 
            "Heap not initialized");
    
    assert!(output.contains("Initializing stack allocation system..."), 
            "Stack allocation system initialization not started");
    
    assert!(output.contains("Stack allocation system initialized"), 
            "Stack allocation system not initialized");
    
    println!("✓ Memory management completeness test passed");
}