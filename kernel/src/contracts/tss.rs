//! TSS contract verification
//!
//! Verifies invariants related to the Task State Segment.


/// Contract: TSS IST entries must point to valid IST stack region (PML4[403])
pub fn verify_ist_stacks_valid() -> Result<(), alloc::string::String> {
    let tss_ptr = crate::gdt::get_tss_ptr();
    if tss_ptr.is_null() {
        return Err("TSS not initialized".into());
    }

    let tss = unsafe { &*tss_ptr };
    let mut errors = alloc::vec::Vec::new();

    // Check IST[0] (double fault stack)
    let ist0 = tss.interrupt_stack_table[0].as_u64();
    if ist0 != 0 {
        let pml4_idx = (ist0 >> 39) & 0x1FF;
        if pml4_idx != 403 {
            errors.push(alloc::format!(
                "IST[0] (double fault) at {:#x} is in PML4[{}], expected PML4[403]",
                ist0, pml4_idx
            ));
        }
    }

    // Check IST[1] (page fault stack)
    let ist1 = tss.interrupt_stack_table[1].as_u64();
    if ist1 != 0 {
        let pml4_idx = (ist1 >> 39) & 0x1FF;
        if pml4_idx != 403 {
            errors.push(alloc::format!(
                "IST[1] (page fault) at {:#x} is in PML4[{}], expected PML4[403]",
                ist1, pml4_idx
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    Ok(())
}

/// Contract: IST[0] (double fault) and IST[1] (page fault) must be different addresses
pub fn verify_ist_separation() -> Result<(), alloc::string::String> {
    let tss_ptr = crate::gdt::get_tss_ptr();
    if tss_ptr.is_null() {
        return Err("TSS not initialized".into());
    }

    let tss = unsafe { &*tss_ptr };

    let ist0 = tss.interrupt_stack_table[0].as_u64();
    let ist1 = tss.interrupt_stack_table[1].as_u64();

    // Both must be set (non-zero)
    if ist0 == 0 {
        return Err("IST[0] (double fault stack) is not set".into());
    }
    if ist1 == 0 {
        return Err("IST[1] (page fault stack) is not set".into());
    }

    // They must be different
    if ist0 == ist1 {
        return Err(alloc::format!(
            "IST[0] and IST[1] both point to {:#x} - they must be separate stacks!",
            ist0
        ));
    }

    Ok(())
}

/// Contract: Verify TSS is properly configured
pub fn verify_tss_config() -> Result<(), alloc::string::String> {
    let tss_ptr = crate::gdt::get_tss_ptr();
    if tss_ptr.is_null() {
        return Err("TSS not initialized".into());
    }

    let tss = unsafe { &*tss_ptr };
    let mut errors = alloc::vec::Vec::new();

    // Verify RSP0 (kernel stack for Ring 3 -> Ring 0 transitions)
    let rsp0 = tss.privilege_stack_table[0].as_u64();
    if rsp0 == 0 {
        errors.push("RSP0 is not set - Ring 3 to Ring 0 transitions will fail".into());
    } else {
        // RSP0 should be in upper canonical address space
        if rsp0 < 0xFFFF_8000_0000_0000 {
            errors.push(alloc::format!(
                "RSP0 ({:#x}) is not in upper canonical space",
                rsp0
            ));
        }
    }

    // Verify IOMAP base (should be disabled by setting it beyond TSS limit)
    let iomap_base = tss.iomap_base;
    let tss_size = core::mem::size_of::<x86_64::structures::tss::TaskStateSegment>() as u16;
    if iomap_base < tss_size {
        // IOMAP is enabled - this could cause issues
        errors.push(alloc::format!(
            "I/O permission bitmap is enabled (iomap_base={} < tss_size={}). \
             This may cause GP faults during CR3 switches.",
            iomap_base, tss_size
        ));
    }

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    Ok(())
}

/// Contract: Verify all TSS invariants
#[allow(dead_code)]
pub fn verify_all_tss_invariants() -> Result<(), alloc::string::String> {
    let mut errors = alloc::vec::Vec::new();

    if let Err(e) = verify_tss_config() {
        errors.push(e);
    }

    if let Err(e) = verify_ist_stacks_valid() {
        errors.push(e);
    }

    if let Err(e) = verify_ist_separation() {
        errors.push(e);
    }

    if !errors.is_empty() {
        return Err(alloc::format!(
            "TSS verification failed:\n  {}",
            errors.join("\n  ")
        ));
    }

    Ok(())
}
