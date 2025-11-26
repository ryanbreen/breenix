use super::{SyscallNumber, SyscallResult};

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so last pushed is at lowest address (where RSP points)
    // Assembly pushes in reverse order: r15 first, rax last
    pub rax: u64, // Syscall number - pushed last, so at RSP+0
    pub rcx: u64, // at RSP+8
    pub rdx: u64, // at RSP+16
    pub rbx: u64, // at RSP+24
    pub rbp: u64, // at RSP+32
    pub rsi: u64, // at RSP+40
    pub rdi: u64, // at RSP+48
    pub r8: u64,  // at RSP+56
    pub r9: u64,  // at RSP+64
    pub r10: u64, // at RSP+72
    pub r11: u64, // at RSP+80
    pub r12: u64, // at RSP+88
    pub r13: u64, // at RSP+96
    pub r14: u64, // at RSP+104
    pub r15: u64, // pushed first, so at RSP+112

    // Interrupt frame (pushed by CPU before our code)
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl SyscallFrame {
    /// Check if this syscall came from userspace
    pub fn is_from_userspace(&self) -> bool {
        // Check CS register - if RPL (bits 0-1) is 3, it's from userspace
        (self.cs & 0x3) == 3
    }

    /// Get syscall number
    pub fn syscall_number(&self) -> u64 {
        self.rax
    }

    /// Get syscall arguments (following System V ABI)
    pub fn args(&self) -> (u64, u64, u64, u64, u64, u64) {
        (self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9)
    }

    /// Set return value
    pub fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }
}

/// Main syscall handler called from assembly
#[no_mangle]
pub extern "C" fn rust_syscall_handler(frame: &mut SyscallFrame) {
    // Raw serial output to detect if syscall handler is called
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x53",      // 'S' for Syscall
            "out dx, al",
            "mov al, 0x43",      // 'C'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }
    
    // Increment preempt count on syscall entry (prevents scheduling during syscall)
    crate::per_cpu::preempt_disable();
    
    // Enhanced syscall entry logging per Cursor requirements
    let from_userspace = frame.is_from_userspace();
    
    // Get current CR3 for logging
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
    }
    
    // Log syscall entry with full frame info for first few syscalls
    use core::sync::atomic::{AtomicU32, Ordering};
    static SYSCALL_ENTRY_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    let entry_count = SYSCALL_ENTRY_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if entry_count < 5 {
        log::info!("R3-SYSCALL ENTRY #{}: CS={:#x} (RPL={}), RIP={:#x}, RSP={:#x}, SS={:#x}, CR3={:#x}",
            entry_count + 1, frame.cs, frame.cs & 3, frame.rip, frame.rsp, frame.ss, cr3);
        
        if from_userspace {
            log::info!("  ✓ Syscall from Ring 3 confirmed (CPL=3)");
        } else {
            log::error!("  ⚠ WARNING: Syscall from Ring {}!", frame.cs & 3);
        }
        
        // Log syscall number and arguments
        log::info!("  Syscall: num={}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            frame.syscall_number(), frame.args().0, frame.args().1, frame.args().2,
            frame.args().3, frame.args().4, frame.args().5);
    }

    // Verify this came from userspace (security check)
    if !from_userspace {
        log::warn!("Syscall from kernel mode - this shouldn't happen!");
        frame.set_return_value(u64::MAX); // Error
        return;
    }

    let syscall_num = frame.syscall_number();
    let args = frame.args();

    // Only log non-write syscalls to reduce noise
    if syscall_num != 1 {
        // 1 is sys_write
        log::trace!(
            "Syscall {} from userspace: RIP={:#x}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            syscall_num,
            frame.rip,
            args.0,
            args.1,
            args.2,
            args.3,
            args.4,
            args.5
        );

        // Debug: Log critical frame values
        log::debug!(
            "Syscall frame before: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x}",
            frame.rip,
            frame.cs,
            frame.rsp,
            frame.ss,
            frame.rax
        );
    }
    
    // Log first few syscalls from userspace with full frame validation
    static SYSCALL_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    if (frame.cs & 3) == 3 {
        let count = SYSCALL_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
        if count < 5 {  // Log first 5 syscalls for verification
            log::info!("Syscall #{} from userspace - full frame validation:", count + 1);
            log::info!("  RIP: {:#x}", frame.rip);
            log::info!("  CS: {:#x} (RPL={})", frame.cs, frame.cs & 3);
            log::info!("  SS: {:#x} (RPL={})", frame.ss, frame.ss & 3);
            log::info!("  RSP: {:#x} (user stack)", frame.rsp);
            log::info!("  RFLAGS: {:#x} (IF={})", frame.rflags, if frame.rflags & 0x200 != 0 { "1" } else { "0" });
            
            // Validate invariants
            if (frame.cs & 3) != 3 {
                log::error!("  ERROR: CS RPL is not 3!");
            }
            if (frame.ss & 3) != 3 {
                log::error!("  ERROR: SS RPL is not 3!");
            }
            if frame.rsp < 0x10000000 || frame.rsp > 0x20000000 {
                log::warn!("  WARNING: RSP {:#x} may be outside expected user range", frame.rsp);
            }
            
            // Get current CR3
            let cr3: u64;
            unsafe {
                core::arch::asm!("mov {}, cr3", out(reg) cr3);
            }
            log::info!("  CR3: {:#x} (current page table)", cr3);
            
            // Try to read the previous 2 bytes (should be 0xcd 0x80 for int 0x80)
            if frame.rip >= 2 {
                unsafe {
                    let int_addr = (frame.rip - 2) as *const u8;
                    // Use volatile read to prevent optimization
                    let byte1 = core::ptr::read_volatile(int_addr);
                    let byte2 = core::ptr::read_volatile(int_addr.offset(1));
                    log::info!("  Previous 2 bytes at RIP-2: {:#02x} {:#02x}", byte1, byte2);
                    if byte1 == 0xcd && byte2 == 0x80 {
                        log::info!("  ✓ Confirmed: int 0x80 instruction detected");
                    } else {
                        log::warn!("  ⚠ Expected int 0x80 (0xcd 0x80) but found {:#02x} {:#02x}", byte1, byte2);
                    }
                }
            }
        }
    }

    // Dispatch to the appropriate syscall handler
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::GetTime) => super::handlers::sys_get_time(),
        Some(SyscallNumber::Fork) => super::handlers::sys_fork_with_frame(frame),
        Some(SyscallNumber::Exec) => super::handlers::sys_exec(args.0, args.1),
        Some(SyscallNumber::GetPid) => super::handlers::sys_getpid(),
        Some(SyscallNumber::GetTid) => super::handlers::sys_gettid(),
        Some(SyscallNumber::ClockGetTime) => {
            let clock_id = args.0 as u32;
            let user_timespec_ptr = args.1 as *mut super::time::Timespec;
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
        None => {
            log::warn!("Unknown syscall number: {} - returning ENOSYS", syscall_num);
            SyscallResult::Err(super::ErrorCode::NoSys as u64)
        }
    };

    // Set return value in RAX
    match result {
        SyscallResult::Ok(val) => frame.set_return_value(val),
        SyscallResult::Err(errno) => {
            // Return -errno in RAX for errors (Linux convention)
            frame.set_return_value((-(errno as i64)) as u64);
        }
    }

    // Debug: Log frame after handling
    if syscall_num != 1 {
        // 1 is sys_write
        log::debug!(
            "Syscall frame after: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x} (return)",
            frame.rip,
            frame.cs,
            frame.rsp,
            frame.ss,
            frame.rax
        );
    }

    // Note: Context switches after sys_yield happen on the next timer interrupt
    
    // CRITICAL FIX: Update TSS.RSP0 before returning to userspace
    // When userspace triggers an interrupt (like int3), the CPU switches to kernel
    // mode and uses TSS.RSP0 as the kernel stack. This must be set correctly!
    let kernel_stack_top = crate::per_cpu::kernel_stack_top();
    if kernel_stack_top.as_u64() != 0 {
        crate::gdt::set_tss_rsp0(kernel_stack_top);
        log::trace!("Updated TSS.RSP0 to {:#x} for userspace return", kernel_stack_top.as_u64());
    } else {
        log::error!("CRITICAL: Cannot set TSS.RSP0 - kernel_stack_top is 0!");
    }
    
    // Flush any pending IRQ logs before returning to userspace
    crate::irq_log::flush_local_try();
    
    // Decrement preempt count on syscall exit
    crate::per_cpu::preempt_enable();
}

// Assembly functions defined in entry.s
extern "C" {
    #[allow(dead_code)]
    pub fn syscall_entry();
    #[allow(dead_code)]
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}

/// Enhanced trace function that logs full IRETQ frame before returning to Ring 3
/// Called from assembly with pointer to IRETQ frame on stack
#[no_mangle]
pub extern "C" fn trace_iretq_to_ring3(frame_ptr: *const u64) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static IRETQ_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

    let count = IRETQ_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 3 {  // Log first 3 transitions for verification
        unsafe {
            // IRETQ frame layout: RIP, CS, RFLAGS, RSP, SS
            let rip = *frame_ptr;
            let cs = *frame_ptr.offset(1);
            let rflags = *frame_ptr.offset(2);
            let rsp = *frame_ptr.offset(3);
            let ss = *frame_ptr.offset(4);

            // Get current CR3
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3);

            // Get current TSS RSP0 value for debugging
            let tss_rsp0 = crate::gdt::get_tss_rsp0();

            // Check IF bit (bit 9) in RFLAGS
            let if_enabled = (rflags & (1 << 9)) != 0;

            log::info!("R3-IRET #{}: rip={:#x}, cs={:#x} (RPL={}), ss={:#x} (RPL={}), rflags={:#x} (IF={}), rsp={:#x}, cr3={:#x}",
                count + 1, rip, cs, cs & 3, ss, ss & 3, rflags, if_enabled as u8, rsp, cr3);
            log::info!("  TSS.RSP0: {:#x}", tss_rsp0);

            // Critical check: IF must be 1 for userspace to receive interrupts
            if !if_enabled {
                log::error!("  🚨 CRITICAL: IF=0 in RFLAGS! Userspace will hang without interrupts!");
                log::error!("  RFLAGS bits: IF(9)={}, TF(8)={}, IOPL(12-13)={}, NT(14)={}",
                    if_enabled as u8,
                    ((rflags >> 8) & 1),
                    ((rflags >> 12) & 3),
                    ((rflags >> 14) & 1));
            }

            // Verify we're returning to Ring 3
            if (cs & 3) == 3 && (ss & 3) == 3 {
                log::info!("  ✓ Confirmed: Returning to Ring 3 (CPL=3) with IF={}", if_enabled as u8);
            } else {
                log::error!("  ⚠ WARNING: Not returning to Ring 3! CS RPL={}, SS RPL={}", cs & 3, ss & 3);
            }

            // ========================================================================
            // COMPREHENSIVE PAGE TABLE VERIFICATION DIAGNOSTICS
            // ========================================================================
            // Purpose: Verify that all critical kernel structures are mapped in the
            // userspace page table BEFORE we execute IRETQ. A triple fault occurs
            // when the CPU can't even enter exception handlers, which means critical
            // structures (IDT, GDT, TSS, IST stacks, kernel code) are not accessible.
            // ========================================================================

            crate::serial_println!("[DIAG:PAGETABLE] ==============================");
            crate::serial_println!("[DIAG:PAGETABLE] PRE-IRETQ PAGE TABLE VERIFICATION");
            crate::serial_println!("[DIAG:PAGETABLE] Verifying mappings in CR3={:#x}", cr3);
            crate::serial_println!("[DIAG:PAGETABLE] ==============================");

            // Get IDT address
            let idtr = x86_64::instructions::tables::sidt();
            crate::serial_println!("[DIAG:PAGETABLE] IDT base: {:#x} (limit: {})",
                idtr.base.as_u64(), idtr.limit);
            crate::serial_println!("[DIAG:PAGETABLE] IDT PML4 index: {}", (idtr.base.as_u64() >> 39) & 0x1FF);

            // Get GDT address
            let gdtr = x86_64::instructions::tables::sgdt();
            crate::serial_println!("[DIAG:PAGETABLE] GDT base: {:#x} (limit: {})",
                gdtr.base.as_u64(), gdtr.limit);
            crate::serial_println!("[DIAG:PAGETABLE] GDT PML4 index: {}", (gdtr.base.as_u64() >> 39) & 0x1FF);

            // Get TSS info
            let (tss_base, tss_rsp0) = crate::gdt::get_tss_info();
            crate::serial_println!("[DIAG:PAGETABLE] TSS base: {:#x}", tss_base);
            crate::serial_println!("[DIAG:PAGETABLE] TSS PML4 index: {}", (tss_base >> 39) & 0x1FF);
            crate::serial_println!("[DIAG:PAGETABLE] TSS.RSP0 (kernel stack): {:#x}", tss_rsp0);
            crate::serial_println!("[DIAG:PAGETABLE] TSS.RSP0 PML4 index: {}", (tss_rsp0 >> 39) & 0x1FF);

            // Get IST stacks from TSS
            let tss_ptr = crate::gdt::get_tss_ptr();
            if !tss_ptr.is_null() {
                let ist0 = (*tss_ptr).interrupt_stack_table[0].as_u64();
                let ist1 = (*tss_ptr).interrupt_stack_table[1].as_u64();
                crate::serial_println!("[DIAG:PAGETABLE] TSS IST[0] (double fault): {:#x}", ist0);
                crate::serial_println!("[DIAG:PAGETABLE] IST[0] PML4 index: {}", (ist0 >> 39) & 0x1FF);
                crate::serial_println!("[DIAG:PAGETABLE] TSS IST[1] (page fault): {:#x}", ist1);
                crate::serial_println!("[DIAG:PAGETABLE] IST[1] PML4 index: {}", (ist1 >> 39) & 0x1FF);
            }

            // Get breakpoint handler address (representative kernel exception handler)
            let handler_addr = crate::interrupts::breakpoint_handler as *const () as u64;
            crate::serial_println!("[DIAG:PAGETABLE] Breakpoint handler (kernel code): {:#x}", handler_addr);
            crate::serial_println!("[DIAG:PAGETABLE] Handler PML4 index: {}", (handler_addr >> 39) & 0x1FF);

            // Get current kernel execution address
            let kernel_rip: u64;
            core::arch::asm!("lea {}, [rip]", out(reg) kernel_rip);
            crate::serial_println!("[DIAG:PAGETABLE] Current kernel RIP: {:#x}", kernel_rip);
            crate::serial_println!("[DIAG:PAGETABLE] Current RIP PML4 index: {}", (kernel_rip >> 39) & 0x1FF);

            // Userspace addresses
            crate::serial_println!("[DIAG:PAGETABLE] Userspace RIP: {:#x}", rip);
            crate::serial_println!("[DIAG:PAGETABLE] Userspace RIP PML4 index: {}", (rip >> 39) & 0x1FF);
            crate::serial_println!("[DIAG:PAGETABLE] Userspace RSP: {:#x}", rsp);
            crate::serial_println!("[DIAG:PAGETABLE] Userspace RSP PML4 index: {}", (rsp >> 39) & 0x1FF);

            // ========================================================================
            // PAGE TABLE WALKING - Check if critical addresses are actually mapped
            // ========================================================================

            // Get physical memory offset for page table walking
            let phys_offset = crate::memory::physical_memory_offset();

            // Helper function to check if an address is mapped
            let check_mapping = |addr: u64, name: &str| {
                // Calculate page table indices
                let pml4_idx = ((addr >> 39) & 0x1FF) as usize;
                let pdpt_idx = ((addr >> 30) & 0x1FF) as usize;
                let pd_idx = ((addr >> 21) & 0x1FF) as usize;
                let pt_idx = ((addr >> 12) & 0x1FF) as usize;

                crate::serial_println!("[DIAG:PAGETABLE] Checking {}: {:#x}", name, addr);

                // Get PML4 (top-level page table)
                let pml4_phys = cr3 & 0xFFFF_FFFF_F000;  // Mask off flags
                let pml4_virt = phys_offset.as_u64() + pml4_phys;
                let pml4 = &*(pml4_virt as *const x86_64::structures::paging::PageTable);

                // Check PML4 entry
                let pml4_entry = &pml4[pml4_idx];
                if pml4_entry.is_unused() {
                    crate::serial_println!("[DIAG:PAGETABLE]   PML4[{}]: UNMAPPED ❌", pml4_idx);
                    return;
                }

                let pml4_flags = pml4_entry.flags();
                let present = pml4_flags.contains(x86_64::structures::paging::PageTableFlags::PRESENT);
                let user = pml4_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                let nx = pml4_flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE);
                crate::serial_println!("[DIAG:PAGETABLE]   PML4[{}]: P={} U={} NX={}",
                    pml4_idx, present as u8, user as u8, nx as u8);

                // Check PDPT entry
                let pdpt_phys = pml4_entry.addr();
                let pdpt_virt = phys_offset.as_u64() + pdpt_phys.as_u64();
                let pdpt = &*(pdpt_virt as *const x86_64::structures::paging::PageTable);
                let pdpt_entry = &pdpt[pdpt_idx];

                if pdpt_entry.is_unused() {
                    crate::serial_println!("[DIAG:PAGETABLE]   PDPT[{}]: UNMAPPED ❌", pdpt_idx);
                    return;
                }

                let pdpt_flags = pdpt_entry.flags();
                let present = pdpt_flags.contains(x86_64::structures::paging::PageTableFlags::PRESENT);
                let user = pdpt_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                let nx = pdpt_flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE);
                crate::serial_println!("[DIAG:PAGETABLE]   PDPT[{}]: P={} U={} NX={}",
                    pdpt_idx, present as u8, user as u8, nx as u8);

                // Check PD entry
                let pd_phys = pdpt_entry.addr();
                let pd_virt = phys_offset.as_u64() + pd_phys.as_u64();
                let pd = &*(pd_virt as *const x86_64::structures::paging::PageTable);
                let pd_entry = &pd[pd_idx];

                if pd_entry.is_unused() {
                    crate::serial_println!("[DIAG:PAGETABLE]   PD[{}]: UNMAPPED ❌", pd_idx);
                    return;
                }

                let pd_flags = pd_entry.flags();
                let present = pd_flags.contains(x86_64::structures::paging::PageTableFlags::PRESENT);
                let user = pd_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                let nx = pd_flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE);
                let huge = pd_flags.contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE);

                if huge {
                    crate::serial_println!("[DIAG:PAGETABLE]   PD[{}]: 2MB HUGE PAGE, P={} U={} NX={} ✓",
                        pd_idx, present as u8, user as u8, nx as u8);
                    return;
                }

                crate::serial_println!("[DIAG:PAGETABLE]   PD[{}]: P={} U={} NX={}",
                    pd_idx, present as u8, user as u8, nx as u8);

                // Check PT entry
                let pt_phys = pd_entry.addr();
                let pt_virt = phys_offset.as_u64() + pt_phys.as_u64();
                let pt = &*(pt_virt as *const x86_64::structures::paging::PageTable);
                let pt_entry = &pt[pt_idx];

                if pt_entry.is_unused() {
                    crate::serial_println!("[DIAG:PAGETABLE]   PT frame={:#x}, PT[{}]: UNMAPPED ❌", pt_phys.as_u64(), pt_idx);
                    return;
                }

                let pt_flags = pt_entry.flags();
                let present = pt_flags.contains(x86_64::structures::paging::PageTableFlags::PRESENT);
                let user = pt_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                let nx = pt_flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE);
                crate::serial_println!("[DIAG:PAGETABLE]   PT[{}]: P={} U={} NX={} ✓",
                    pt_idx, present as u8, user as u8, nx as u8);
            };

            // Check critical kernel structures
            check_mapping(idtr.base.as_u64(), "IDT");
            check_mapping(gdtr.base.as_u64(), "GDT");
            check_mapping(tss_base, "TSS");
            // Check TSS.RSP0 - 16 (stack top points past the last valid byte)
            check_mapping(tss_rsp0.wrapping_sub(16), "TSS.RSP0 (kernel stack)");

            // Check IST stacks (stack tops point past the last valid byte)
            if !tss_ptr.is_null() {
                let ist0 = (*tss_ptr).interrupt_stack_table[0].as_u64();
                let ist1 = (*tss_ptr).interrupt_stack_table[1].as_u64();
                check_mapping(ist0.wrapping_sub(16), "IST[0] (double fault stack)");
                check_mapping(ist1.wrapping_sub(16), "IST[1] (page fault stack)");
            }

            // Check kernel exception handler
            check_mapping(handler_addr, "Breakpoint handler (kernel code)");

            // Check userspace addresses
            check_mapping(rip, "Userspace RIP");
            check_mapping(rsp, "Userspace RSP");

            crate::serial_println!("[DIAG:PAGETABLE] ==============================");
            crate::serial_println!("[DIAG:PAGETABLE] VERIFICATION COMPLETE");
            crate::serial_println!("[DIAG:PAGETABLE] ==============================");
        }
    }
}
