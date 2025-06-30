//! Userspace context switching
//!
//! This module handles switching from kernel to userspace contexts.

use x86_64::VirtAddr;
use x86_64::registers::segmentation::{CS, Segment};

/// Switch to a userspace thread using IRETQ
/// 
/// This function never returns - it jumps to userspace
pub unsafe fn switch_to_userspace(
    entry_point: VirtAddr,
    stack_pointer: VirtAddr,
    user_code_segment: u16,
    user_data_segment: u16,
) -> ! {
    log::info!("Switching to userspace: entry={:#x}, stack={:#x}, cs={:#x}, ss={:#x}", 
        entry_point, stack_pointer, user_code_segment, user_data_segment);
    
    // Ensure segments are valid
    log::debug!("Current CS: {:#x}", CS::get_reg().0);
    
    // Set data segments - but NOT SS yet, that will be done by IRETQ
    core::arch::asm!(
        "mov ax, {0:x}",
        "mov ds, ax",
        "mov es, ax",
        in(reg) user_data_segment,
    );
    
    log::debug!("About to execute IRETQ with:");
    log::debug!("  RIP = {:#x}", entry_point.as_u64());
    log::debug!("  CS  = {:#x}", user_code_segment);
    log::debug!("  RSP = {:#x}", stack_pointer.as_u64());
    log::debug!("  SS  = {:#x}", user_data_segment);
    log::debug!("  RFLAGS = {:#x}", 0x202u64);
    
    // Verify selectors have Ring 3 RPL
    assert!((user_code_segment & 3) == 3, "CS must have RPL=3");
    assert!((user_data_segment & 3) == 3, "SS must have RPL=3");
    
    // Get current stack pointer to see where we are
    let current_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) current_rsp);
    }
    log::debug!("Current kernel RSP before IRETQ: {:#x}", current_rsp);
    
    // Build IRETQ frame on the kernel stack
    // The IRETQ instruction expects:
    // [RSP+32] SS
    // [RSP+24] RSP
    // [RSP+16] RFLAGS
    // [RSP+8]  CS
    // [RSP+0]  RIP
    
    core::arch::asm!(
        // Push the IRETQ frame
        "push {ss}",         // SS
        "push {rsp}",        // RSP
        "push 0x202",        // RFLAGS (IF=1, bit 9 = interrupt enable)
        "push {cs}",         // CS
        "push {rip}",        // RIP
        
        // Switch to userspace
        "iretq",
        
        ss = in(reg) user_data_segment as u64,
        rsp = in(reg) stack_pointer.as_u64(),
        cs = in(reg) user_code_segment as u64,
        rip = in(reg) entry_point.as_u64(),
        options(noreturn)
    );
}

/// Switch to userspace thread from scheduler
pub unsafe fn scheduler_switch_to_userspace(thread: &super::thread::Thread) -> ! {
    // Get segment selectors from GDT
    let user_cs = crate::gdt::USER_CODE_SELECTOR.0;
    let user_ds = crate::gdt::USER_DATA_SELECTOR.0;
    
    // Ensure Ring 3 RPL bits are set
    let user_cs_ring3 = user_cs | 3;
    let user_ds_ring3 = user_ds | 3;
    
    switch_to_userspace(
        VirtAddr::new(thread.context.rip),
        VirtAddr::new(thread.context.rsp),
        user_cs_ring3,
        user_ds_ring3,
    )
}