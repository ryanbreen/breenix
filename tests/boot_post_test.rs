//! Kernel POST test using shared QEMU infrastructure

mod shared_qemu;
use shared_qemu::get_kernel_output;

#[test]
fn test_kernel_post_with_file_output() {
    println!("\n🧪 Breenix Kernel POST Test (Shared QEMU)");
    println!("==========================================\n");
    
    // Get kernel output from shared QEMU instance
    let output = get_kernel_output();
    
    // POST checks
    println!("\n📋 POST Results:");
    println!("================\n");
    
    let post_checks = [
        ("CPU", "Kernel entry point reached", "Basic CPU execution"),
        ("Serial Port", "Serial port initialized", "Serial I/O communication"),  
        ("Display", "Logger fully initialized", "Framebuffer graphics"),
        ("GDT", "GDT initialized", "Global Descriptor Table"),
        ("IDT", "IDT loaded successfully", "Interrupt Descriptor Table"),
        ("Memory Detection", "Physical memory offset available", "Memory mapping"),
        ("Frame Allocator", "Frame allocator initialized", "Physical memory management"),
        ("Paging", "Page table initialized", "Virtual memory management"),
        ("Heap", "Heap initialized", "Dynamic memory allocation"),
        ("Memory System", "Memory management initialized", "Complete memory subsystem"),
        ("Timer/RTC", "Timer initialized", "System timer and RTC"),
        ("Keyboard", "Keyboard queue initialized", "Keyboard controller"),
        ("PIC", "PIC initialized", "Programmable Interrupt Controller"),
        ("Interrupts", "Interrupts enabled!", "Interrupt system active"),
    ];
    
    let mut passed = 0;
    let mut failed = Vec::new();
    
    for (subsystem, check_string, description) in &post_checks {
        print!("  {:.<18} ", subsystem);
        if output.contains(check_string) {
            println!("✅ PASS - {}", description);
            passed += 1;
        } else {
            println!("❌ FAIL - {}", description);
            failed.push((*subsystem, *check_string));
        }
    }
    
    println!("\n================");
    println!("Summary: {}/{} subsystems passed POST", passed, post_checks.len());
    println!("================\n");
    
    // Check if POST completion marker is present
    if output.contains("🎯 KERNEL_POST_TESTS_COMPLETE 🎯") {
        println!("✅ POST completion marker found - kernel reached expected state");
    } else {
        println!("⚠️  POST completion marker not found - kernel may not have fully initialized");
    }
    
    if !failed.is_empty() {
        eprintln!("❌ POST FAILED - The following subsystems did not initialize:");
        for (subsystem, _) in &failed {
            eprintln!("   - {}", subsystem);
        }
        eprintln!("\nFirst 50 lines of kernel output:");
        eprintln!("--------------------------------");
        for line in output.lines().take(50) {
            eprintln!("{}", line);
        }
        
        panic!("Kernel POST failed - {} subsystems did not initialize", failed.len());
    }
    
    println!("✅ All POST checks passed - kernel is healthy!\n");
}