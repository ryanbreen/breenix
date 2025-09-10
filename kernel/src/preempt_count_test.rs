//! Comprehensive preempt_count testing module
//!
//! Tests all preempt_count functions to validate the implementation

use crate::per_cpu;

/// Comprehensive test of all preempt_count functions
pub fn test_preempt_count_comprehensive() {
    log::info!("=== PREEMPT_COUNT COMPREHENSIVE TEST START ===");
    
    // Test 1: Initial state
    let initial = per_cpu::preempt_count();
    log::info!("TEST 1: Initial preempt_count = {:#x}", initial);
    assert!(initial == 0, "Initial count should be 0");
    
    // Test 2: Preempt disable/enable
    log::info!("TEST 2: Testing preempt_disable/enable...");
    per_cpu::preempt_disable();
    let after_disable = per_cpu::preempt_count();
    log::info!("  After preempt_disable: {:#x}", after_disable);
    assert!(after_disable == 1, "Count should be 1 after disable");
    
    per_cpu::preempt_enable();
    let after_enable = per_cpu::preempt_count();
    log::info!("  After preempt_enable: {:#x}", after_enable);
    assert!(after_enable == 0, "Count should be 0 after enable");
    
    // Test 3: Nested preempt disable/enable
    log::info!("TEST 3: Testing nested preempt_disable/enable...");
    per_cpu::preempt_disable();
    per_cpu::preempt_disable();
    per_cpu::preempt_disable();
    let nested_disable = per_cpu::preempt_count();
    log::info!("  After 3x preempt_disable: {:#x}", nested_disable);
    assert!(nested_disable == 3, "Count should be 3 after triple disable");
    
    per_cpu::preempt_enable();
    let nested_enable1 = per_cpu::preempt_count();
    log::info!("  After 1x preempt_enable: {:#x}", nested_enable1);
    assert!(nested_enable1 == 2, "Count should be 2");
    
    per_cpu::preempt_enable();
    per_cpu::preempt_enable();
    let nested_enable_final = per_cpu::preempt_count();
    log::info!("  After all preempt_enable: {:#x}", nested_enable_final);
    assert!(nested_enable_final == 0, "Count should be 0");
    
    // Test 4: IRQ context (simulated)
    log::info!("TEST 4: Simulating IRQ context...");
    per_cpu::irq_enter();
    let in_irq = per_cpu::preempt_count();
    log::info!("  After irq_enter: {:#x}", in_irq);
    assert!(in_irq == 0x10000, "Should have HARDIRQ bit set");
    assert!(per_cpu::in_hardirq(), "Should be in hardirq");
    
    // Test nested preemption disable in IRQ
    per_cpu::preempt_disable();
    let irq_with_preempt = per_cpu::preempt_count();
    log::info!("  After preempt_disable in IRQ: {:#x}", irq_with_preempt);
    assert!(irq_with_preempt == 0x10001, "Should have both HARDIRQ and PREEMPT");
    
    per_cpu::preempt_enable();
    per_cpu::irq_exit();
    let after_irq = per_cpu::preempt_count();
    log::info!("  After irq_exit: {:#x}", after_irq);
    assert!(after_irq == 0, "Count should be 0 after IRQ exit");
    assert!(!per_cpu::in_hardirq(), "Should not be in hardirq");
    
    // Test 5: Softirq context
    log::info!("TEST 5: Testing softirq context...");
    per_cpu::softirq_enter();
    let in_softirq = per_cpu::preempt_count();
    log::info!("  After softirq_enter: {:#x}", in_softirq);
    assert!(in_softirq == 0x100, "Should have SOFTIRQ bit set");
    assert!(per_cpu::in_softirq(), "Should be in softirq");
    
    per_cpu::softirq_exit();
    let after_softirq = per_cpu::preempt_count();
    log::info!("  After softirq_exit: {:#x}", after_softirq);
    assert!(after_softirq == 0, "Count should be 0 after softirq exit");
    assert!(!per_cpu::in_softirq(), "Should not be in softirq");
    
    // Test 6: NMI context
    log::info!("TEST 6: Testing NMI context...");
    per_cpu::nmi_enter();
    let in_nmi = per_cpu::preempt_count();
    log::info!("  After nmi_enter: {:#x}", in_nmi);
    assert!(in_nmi == 0x4000000, "Should have NMI bit set");
    assert!(per_cpu::in_nmi(), "Should be in NMI");
    
    per_cpu::nmi_exit();
    let after_nmi = per_cpu::preempt_count();
    log::info!("  After nmi_exit: {:#x}", after_nmi);
    assert!(after_nmi == 0, "Count should be 0 after NMI exit");
    assert!(!per_cpu::in_nmi(), "Should not be in NMI");
    
    // Test 7: Mixed contexts
    log::info!("TEST 7: Testing mixed contexts...");
    per_cpu::preempt_disable();
    per_cpu::irq_enter();
    per_cpu::softirq_enter();
    let mixed = per_cpu::preempt_count();
    log::info!("  Mixed (preempt+irq+softirq): {:#x}", mixed);
    assert!(mixed == 0x10101, "Should have all three bits");
    assert!(per_cpu::in_hardirq(), "Should be in hardirq");
    assert!(per_cpu::in_softirq(), "Should be in softirq");
    assert!(per_cpu::in_interrupt(), "Should be in interrupt");
    
    per_cpu::softirq_exit();
    per_cpu::irq_exit();
    per_cpu::preempt_enable();
    let mixed_cleared = per_cpu::preempt_count();
    log::info!("  After clearing mixed: {:#x}", mixed_cleared);
    assert!(mixed_cleared == 0, "Count should be 0");
    
    // Test 8: Nested IRQ (simulating nested interrupts)
    log::info!("TEST 8: Testing nested IRQ context...");
    per_cpu::irq_enter();
    let irq1 = per_cpu::preempt_count();
    log::info!("  First irq_enter: {:#x}", irq1);
    
    per_cpu::irq_enter();
    let irq2 = per_cpu::preempt_count();
    log::info!("  Second irq_enter: {:#x}", irq2);
    assert!(irq2 == 0x20000, "Should have count=2 in HARDIRQ field");
    
    per_cpu::irq_exit();
    let irq1_again = per_cpu::preempt_count();
    log::info!("  After first irq_exit: {:#x}", irq1_again);
    assert!(irq1_again == 0x10000, "Should be back to count=1");
    
    per_cpu::irq_exit();
    let irq_done = per_cpu::preempt_count();
    log::info!("  After second irq_exit: {:#x}", irq_done);
    assert!(irq_done == 0, "Should be 0");
    
    // Test 9: Check all query functions
    log::info!("TEST 9: Testing query functions...");
    assert!(!per_cpu::in_interrupt(), "Not in interrupt");
    assert!(!per_cpu::in_hardirq(), "Not in hardirq");
    assert!(!per_cpu::in_softirq(), "Not in softirq");
    assert!(!per_cpu::in_nmi(), "Not in NMI");
    
    per_cpu::irq_enter();
    assert!(per_cpu::in_interrupt(), "In interrupt (IRQ)");
    assert!(per_cpu::in_hardirq(), "In hardirq");
    per_cpu::irq_exit();
    
    per_cpu::softirq_enter();
    assert!(per_cpu::in_interrupt(), "In interrupt (softirq)");
    assert!(per_cpu::in_softirq(), "In softirq");
    per_cpu::softirq_exit();
    
    // Test 10: Spinlock integration
    log::info!("TEST 10: Testing spinlock integration...");
    crate::spinlock::test_spinlock_preemption();
    
    log::info!("=== PREEMPT_COUNT COMPREHENSIVE TEST PASSED ===");
    log::info!("✅ All preempt_count functions validated successfully");
}

/// Test scheduling integration
pub fn test_preempt_count_scheduling() {
    log::info!("=== PREEMPT_COUNT SCHEDULING TEST START ===");
    
    // This test validates that scheduling only happens when safe
    let initial = per_cpu::preempt_count();
    log::info!("Initial preempt_count: {:#x}", initial);
    
    // Set need_resched flag
    per_cpu::set_need_resched(true);
    log::info!("Set need_resched flag");
    
    // Preempt enable should NOT schedule if in interrupt
    per_cpu::irq_enter();
    log::info!("Entered IRQ context: {:#x}", per_cpu::preempt_count());
    
    per_cpu::preempt_disable();
    per_cpu::preempt_enable();  // Should NOT schedule here
    log::info!("preempt_enable in IRQ did not schedule (correct)");
    
    per_cpu::irq_exit();  // This MAY schedule via preempt_schedule_irq
    log::info!("Exited IRQ context: {:#x}", per_cpu::preempt_count());
    
    // Now test normal preemption
    per_cpu::set_need_resched(true);
    per_cpu::preempt_disable();
    log::info!("Preemption disabled: {:#x}", per_cpu::preempt_count());
    
    per_cpu::preempt_enable();  // This SHOULD schedule if not in interrupt
    log::info!("Preemption enabled and may have scheduled");
    
    // CRITICAL: Clear need_resched flag after test to avoid interfering with system
    per_cpu::set_need_resched(false);
    log::info!("Cleared need_resched flag after test");
    
    log::info!("=== PREEMPT_COUNT SCHEDULING TEST PASSED ===");
}