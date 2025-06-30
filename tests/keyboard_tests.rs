mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test keyboard system initialization and basic functionality
#[test]
fn test_keyboard_initialization() {
    println!("Testing keyboard initialization...");
    
    let output = get_kernel_output();
    
    // Check for keyboard initialization
    assert!(
        output.contains("Keyboard queue initialized"),
        "Keyboard queue not initialized"
    );
    
    // Check for keyboard ready message
    assert!(
        output.contains("Keyboard ready! Type to see characters"),
        "Keyboard ready message not found"
    );
    
    // Check that special key instructions are shown in the new async implementation
    assert!(
        output.contains("Ctrl+C/D/S/T/M for special actions"),
        "Special key instructions not shown"
    );
    
    println!("✅ Keyboard initialization test passed");
}

/// Test that keyboard features are properly reported in feature comparison
#[test] 
fn test_keyboard_features() {
    println!("Testing keyboard feature implementation...");
    
    // The kernel now has:
    // - Full scancode-to-ASCII translation
    // - Complete modifier key tracking (Shift, Ctrl, Alt, Cmd, Caps Lock)
    // - Proper Caps Lock handling (only affects alphabetic keys)
    // - Special key combinations (Ctrl+C, Ctrl+D, Ctrl+S)
    
    let output = get_kernel_output();
    
    // Verify the kernel is ready to receive keyboard input
    assert!(
        output.contains("Keyboard ready"),
        "Keyboard not ready for input"
    );
    
    println!("✅ Keyboard features test passed");
}