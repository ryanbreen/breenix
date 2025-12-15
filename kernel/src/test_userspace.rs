/// Minimal int3 trampoline test for reaching Ring 3
/// Based on Cursor's guidance for simplest possible userspace test

use core::arch::naked_asm;
use x86_64::VirtAddr;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};

/// Test reaching Ring 3 with minimal setup - just an int3 instruction
#[allow(dead_code)]
pub fn test_minimal_userspace() {
    crate::serial_println!("=== MINIMAL INT3 TRAMPOLINE TEST ===");
    
    // 1. Map a single page at 0x400000 with just int3 (0xCC)
    let user_page = Page::<Size4KiB>::containing_address(VirtAddr::new(0x400000));
    
    // FOR TEST: Map directly into kernel page table since we're not switching CR3
    // This is temporary for debugging - normally would use process page table
    {
        use x86_64::structures::paging::Mapper;
        use x86_64::structures::paging::mapper::MapToError;
        
        // Get the kernel's active page table using x86_64 crate
        use x86_64::structures::paging::OffsetPageTable;
        let phys_offset = crate::memory::physical_memory_offset();
        let level_4_table = unsafe {
            use x86_64::registers::control::Cr3;
            let (level_4_table_frame, _) = Cr3::read();
            let phys = level_4_table_frame.start_address();
            let virt = phys_offset + phys.as_u64();
            &mut *virt.as_mut_ptr::<x86_64::structures::paging::PageTable>()
        };
        let mut kernel_mapper = unsafe { OffsetPageTable::new(level_4_table, phys_offset) };
        
        // Allocate a frame for the int3 instruction
        let frame = crate::memory::frame_allocator::allocate_frame()
            .expect("Failed to allocate frame for int3 test");
        
        // Map with USER_ACCESSIBLE so Ring 3 can execute it
        let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        
        // Try to map the page
        unsafe {
            match kernel_mapper.map_to(user_page, frame, flags, &mut crate::memory::frame_allocator::GlobalFrameAllocator) {
                Ok(flush) => {
                    flush.flush();
                    crate::serial_println!("✓ Mapped int3 at 0x400000 in KERNEL page table");
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    crate::serial_println!("⚠️ Page 0x400000 already mapped in kernel table");
                }
                Err(e) => {
                    panic!("Failed to map int3 page in kernel table: {:?}", e);
                }
            }
        }
        
        // Write int3 (0xCC) followed by safe instructions
        unsafe {
            let phys_offset = crate::memory::physical_memory_offset();
            let virt_addr = phys_offset + frame.start_address().as_u64();
            let ptr = virt_addr.as_mut_ptr::<u8>();
            
            // 0x400000: INT3 (0xCC) - our breakpoint trigger
            *ptr.add(0) = 0xCC;
            // 0x400001: NOP (0x90) - safe instruction after breakpoint
            *ptr.add(1) = 0x90;
            // 0x400002: NOP (0x90) - another safe instruction
            *ptr.add(2) = 0x90;  
            // 0x400003: INT3 (0xCC) - another breakpoint to verify flow
            *ptr.add(3) = 0xCC;
        }
        
        crate::serial_println!("✓ Wrote int3 instruction to 0x400000");
    }
    
    // Also map into process page table for CR3 switch
    if let Some(mut manager) = crate::process::try_manager() {
        if let Some(manager) = manager.as_mut() {
            // Get process 1's page table (hello_world process)
            if let Some(process) = manager.get_process_mut(crate::process::ProcessId::new(1)) {
                if let Some(ref mut page_table) = process.page_table {
                    // Use the same frame that we mapped in kernel page table
                    // Get the frame from kernel mapping
                    let frame = {
                        use x86_64::structures::paging::Mapper;
                        use x86_64::structures::paging::OffsetPageTable;
                        
                        let phys_offset = crate::memory::physical_memory_offset();
                        let level_4_table = unsafe {
                            use x86_64::registers::control::Cr3;
                            let (level_4_table_frame, _) = Cr3::read();
                            let phys = level_4_table_frame.start_address();
                            let virt = phys_offset + phys.as_u64();
                            &mut *virt.as_mut_ptr::<x86_64::structures::paging::PageTable>()
                        };
                        let kernel_mapper = unsafe { OffsetPageTable::new(level_4_table, phys_offset) };
                        
                        // Translate the page to get its frame
                        kernel_mapper.translate_page(user_page)
                            .expect("int3 page should be mapped in kernel table")
                            .start_address()
                            .as_u64()
                    };
                    
                    // Map the same frame in process page table
                    let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                    use x86_64::PhysAddr;
                    let frame = x86_64::structures::paging::PhysFrame::containing_address(PhysAddr::new(frame));
                    page_table.map_page(user_page, frame, flags)
                        .expect("Failed to map user page in process table");

                    // Verify the mapping worked
                    if let Some(phys_addr) = page_table.translate_page(user_page.start_address()) {
                        if phys_addr == frame.start_address() {
                            crate::serial_println!("✓ Mapped int3 at 0x400000 in process page table -> {:#x}",
                                frame.start_address().as_u64());
                        } else {
                            crate::serial_println!("ERROR: page mapped to wrong frame in process table!");
                        }
                    } else {
                        crate::serial_println!("ERROR: page not mapped in process table after map_page!");
                    }

                    use x86_64::instructions::tlb;
                    tlb::flush(user_page.start_address());
                } else {
                    crate::serial_println!("⚠️ Process has no page table!");
                }
            } else {
                crate::serial_println!("⚠️ Process 1 not found!");
            }
        }
    } else {
        crate::serial_println!("⚠️ Could not get process manager!");
    }
    
    // 2. Map a user stack page at 0x800000 - also in kernel page table for this test
    let stack_page = Page::<Size4KiB>::containing_address(VirtAddr::new(0x800000));
    
    // Map stack in kernel page table
    {
        use x86_64::structures::paging::Mapper;
        use x86_64::structures::paging::mapper::MapToError;
        use x86_64::structures::paging::OffsetPageTable;
        
        let phys_offset = crate::memory::physical_memory_offset();
        let level_4_table = unsafe {
            use x86_64::registers::control::Cr3;
            let (level_4_table_frame, _) = Cr3::read();
            let phys = level_4_table_frame.start_address();
            let virt = phys_offset + phys.as_u64();
            &mut *virt.as_mut_ptr::<x86_64::structures::paging::PageTable>()
        };
        let mut kernel_mapper = unsafe { OffsetPageTable::new(level_4_table, phys_offset) };
        
        let stack_frame = crate::memory::frame_allocator::allocate_frame()
            .expect("Failed to allocate stack frame");
        
        // Map stack with User|Present|Writable|NX flags  
        let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | 
                   PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        unsafe {
            match kernel_mapper.map_to(stack_page, stack_frame, flags, &mut crate::memory::frame_allocator::GlobalFrameAllocator) {
                Ok(flush) => {
                    flush.flush();
                    crate::serial_println!("✓ Mapped user stack at 0x800000 in KERNEL page table");
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    crate::serial_println!("⚠️ Stack page 0x800000 already mapped in kernel table");
                }
                Err(e) => {
                    panic!("Failed to map stack page in kernel table: {:?}", e);
                }
            }
        }
    }
    
    // Also map stack in process page table
    if let Some(mut manager) = crate::process::try_manager() {
        if let Some(manager) = manager.as_mut() {
            if let Some(process) = manager.get_process_mut(crate::process::ProcessId::new(1)) {
                if let Some(ref mut page_table) = process.page_table {
                    // Get the stack frame from kernel mapping
                    let stack_frame_addr = {
                        use x86_64::structures::paging::Mapper;
                        use x86_64::structures::paging::OffsetPageTable;
                        
                        let phys_offset = crate::memory::physical_memory_offset();
                        let level_4_table = unsafe {
                            use x86_64::registers::control::Cr3;
                            let (level_4_table_frame, _) = Cr3::read();
                            let phys = level_4_table_frame.start_address();
                            let virt = phys_offset + phys.as_u64();
                            &mut *virt.as_mut_ptr::<x86_64::structures::paging::PageTable>()
                        };
                        let kernel_mapper = unsafe { OffsetPageTable::new(level_4_table, phys_offset) };
                        
                        kernel_mapper.translate_page(stack_page)
                            .expect("stack page should be mapped in kernel table")
                            .start_address()
                            .as_u64()
                    };

                    // Map stack with User|Present|Writable|NX flags
                    let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE |
                               PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
                    use x86_64::PhysAddr;
                    let stack_frame = x86_64::structures::paging::PhysFrame::containing_address(PhysAddr::new(stack_frame_addr));
                    page_table.map_page(stack_page, stack_frame, flags)
                        .expect("Failed to map user stack in process table");
                    use x86_64::instructions::tlb;
                    tlb::flush(stack_page.start_address());
                    
                    crate::serial_println!("✓ Mapped user stack at 0x800000 in process page table");
                }
            }
        }
    }
    
    // 3. Set up TSS RSP0 for exception handling from Ring 3
    // This is critical - without it, CPU can't switch stacks on exception
    {
        // Allocate a kernel stack for exception handling
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack()
            .expect("Failed to allocate kernel stack for TSS RSP0");
        let kernel_stack_top = kernel_stack.top();
        
        crate::serial_println!("Setting TSS RSP0 to {:#x}", kernel_stack_top.as_u64());

        // Try the per_cpu method first
        crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());

        // Also set it directly via GDT module's public function
        crate::gdt::set_tss_rsp0(kernel_stack_top);
        crate::serial_println!("Set TSS.RSP0 directly via GDT module");
        
        // Verify TSS.RSP0 was actually set
        let actual_rsp0 = crate::gdt::get_tss_rsp0();
        crate::serial_println!("TSS RSP0 readback: {:#x}", actual_rsp0);
        if actual_rsp0 != kernel_stack_top.as_u64() {
            panic!("TSS.RSP0 not set correctly! Expected {:#x}, got {:#x}", 
                   kernel_stack_top.as_u64(), actual_rsp0);
        }
        
        // Also verify the TSS is the one that's loaded
        let (tss_base, tss_rsp0) = crate::gdt::get_tss_info();
        crate::serial_println!("Active TSS at {:#x}, RSP0={:#x}", tss_base, tss_rsp0);
        
        crate::serial_println!("✓ TSS RSP0 configured for Ring 3 → Ring 0 transitions");
        
        // Keep kernel_stack alive for the duration of the test
        core::mem::forget(kernel_stack);
    }
    
    // 4. Switch to process page table
    crate::serial_println!("Switching to process page table...");
    
    if let Some(mut manager) = crate::process::try_manager() {
        if let Some(manager) = manager.as_mut() {
            if let Some(process) = manager.get_process_mut(crate::process::ProcessId::new(1)) {
                if let Some(ref page_table) = process.page_table {
                    let cr3_frame = page_table.level_4_frame();
                    
                    unsafe {
                        use x86_64::registers::control::{Cr3, Cr3Flags};
                        
                        // Log the CR3 switch
                        let (current_cr3, _) = Cr3::read();
                        crate::serial_println!("Switching CR3: {:#x} -> {:#x}", 
                            current_cr3.start_address().as_u64(),
                            cr3_frame.start_address().as_u64());
                        
                        // Output marker before CR3 switch
                        core::arch::asm!(
                            "mov dx, 0x3F8",
                            "mov al, 0x55",  // 'U' for userspace test with process CR3
                            "out dx, al",
                            options(nostack, nomem, preserves_flags)
                        );
                        
                        Cr3::write(cr3_frame, Cr3Flags::empty());
                        
                        crate::serial_println!("✓ Switched to process CR3");
                    }
                } else {
                    panic!("Process has no page table!");
                }
            } else {
                panic!("Process 1 not found!");
            }
        }
    } else {
        panic!("Could not get process manager!");
    }
    
    // 5. Jump to Ring 3 with iretq
    unsafe {
        jump_to_userspace();
    }
}

/// Perform the actual jump to userspace using iretq
#[unsafe(naked)]
unsafe extern "C" fn jump_to_userspace() -> ! {
    naked_asm!(
        // Disable interrupts during transition
        "cli",
        
        // Output marker that we're about to iretq
        "mov dx, 0x3F8",
        "mov al, 0x49",      // 'I' for iretq
        "out dx, al",
        
        // Push interrupt frame for iretq
        // Order: SS, RSP, RFLAGS, CS, RIP
        "push 0x2b",         // SS: user data selector (0x28 | 3)
        "push 0x800ff8",     // RSP: just below top of user stack (inside mapped region)
        "push 0x2",          // RFLAGS: bit 1 must be 1, IF=0 (no interrupts)
        "push 0x33",         // CS: user code selector (0x30 | 3)  
        "push 0x400000",     // RIP: int3 location
        
        // Output final marker before iretq
        "mov dx, 0x3F8",
        "mov al, 0x52",      // 'R' for Ring 3
        "out dx, al",
        
        // Jump to userspace!
        "iretq",
        
    )
}