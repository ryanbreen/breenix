//! Contract test runner
//!
//! Executes all contract tests and reports results.

#[cfg(feature = "testing")]
use x86_64::{registers::control::Cr3, structures::paging::PageTable};

#[cfg(feature = "testing")]
use crate::contracts::{page_table, kernel_stack, tss};

/// Run all contract tests and report results
#[cfg(feature = "testing")]
pub fn run_all_contracts() -> (usize, usize) {
    log::info!("=== Running Contract Tests ===");

    let mut total_passed = 0;
    let mut total_failed = 0;

    // Test current page table
    let (passed, failed) = test_current_page_table();
    total_passed += passed;
    total_failed += failed;

    // Test master PML4
    let (passed, failed) = test_master_pml4();
    total_passed += passed;
    total_failed += failed;

    // Test TSS invariants
    let (passed, failed) = test_tss_invariants();
    total_passed += passed;
    total_failed += failed;

    // Test all processes
    let (passed, failed) = test_all_processes();
    total_passed += passed;
    total_failed += failed;

    log::info!("=== Contract Tests Complete: {} passed, {} failed ===",
               total_passed, total_failed);

    (total_passed, total_failed)
}

/// Run contracts on current CR3 page table
#[cfg(feature = "testing")]
pub fn test_current_page_table() -> (usize, usize) {
    log::info!("Testing current page table (CR3)...");

    let mut passed = 0;
    let mut failed = 0;

    let phys_offset = crate::memory::physical_memory_offset();
    let (current_frame, _) = Cr3::read();

    let pml4_virt = phys_offset + current_frame.start_address().as_u64();
    let pml4 = unsafe { &*(pml4_virt.as_ptr() as *const PageTable) };

    // Test 1: Kernel/IST frame separation
    match page_table::verify_kernel_ist_frame_separation(pml4) {
        Ok(()) => {
            log::info!("  [PASS] PML4[402]/[403] frame separation");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] PML4[402]/[403] frame separation: {}", e);
            failed += 1;
        }
    }

    // Test 2: Kernel code mapping
    match page_table::verify_kernel_code_mapping(pml4) {
        Ok(()) => {
            log::info!("  [PASS] PML4[2] (kernel code mapping)");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] PML4[2] (kernel code mapping): {}", e);
            failed += 1;
        }
    }

    // Test 3: Stack regions present
    match page_table::verify_stack_regions_present(pml4) {
        Ok(()) => {
            log::info!("  [PASS] Stack regions present");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] Stack regions present: {}", e);
            failed += 1;
        }
    }

    // Test 4: TSS RSP0 valid
    match kernel_stack::verify_tss_rsp0_valid() {
        Ok(()) => {
            log::info!("  [PASS] TSS RSP0 valid");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] TSS RSP0 valid: {}", e);
            failed += 1;
        }
    }

    (passed, failed)
}

/// Run contracts on master kernel PML4
#[cfg(feature = "testing")]
pub fn test_master_pml4() -> (usize, usize) {
    log::info!("Testing master kernel PML4...");

    let mut passed = 0;
    let mut failed = 0;

    let phys_offset = crate::memory::physical_memory_offset();

    match crate::memory::kernel_page_table::master_kernel_pml4() {
        Some(master_frame) => {
            let master_virt = phys_offset + master_frame.start_address().as_u64();
            let master_pml4 = unsafe { &*(master_virt.as_ptr() as *const PageTable) };

            // Test 1: All kernel entries valid
            match page_table::verify_all_kernel_entries(master_pml4) {
                Ok(()) => {
                    log::info!("  [PASS] Master PML4 all kernel entries valid");
                    passed += 1;
                }
                Err(e) => {
                    log::error!("  [FAIL] Master PML4 kernel entries: {}", e);
                    failed += 1;
                }
            }

            // Test 2: Frame separation in master
            match page_table::verify_kernel_ist_frame_separation(master_pml4) {
                Ok(()) => {
                    log::info!("  [PASS] Master PML4[402]/[403] frame separation");
                    passed += 1;
                }
                Err(e) => {
                    log::error!("  [FAIL] Master PML4[402]/[403] frame separation: {}", e);
                    failed += 1;
                }
            }
        }
        None => {
            log::warn!("  [SKIP] Master kernel PML4 not available");
        }
    }

    (passed, failed)
}

/// Run contracts on TSS invariants
#[cfg(feature = "testing")]
pub fn test_tss_invariants() -> (usize, usize) {
    log::info!("Testing TSS invariants...");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: TSS configuration
    match tss::verify_tss_config() {
        Ok(()) => {
            log::info!("  [PASS] TSS configuration");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] TSS configuration: {}", e);
            failed += 1;
        }
    }

    // Test 2: IST stacks valid
    match tss::verify_ist_stacks_valid() {
        Ok(()) => {
            log::info!("  [PASS] IST stacks valid");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] IST stacks valid: {}", e);
            failed += 1;
        }
    }

    // Test 3: IST separation
    match tss::verify_ist_separation() {
        Ok(()) => {
            log::info!("  [PASS] IST[0]/[1] separation");
            passed += 1;
        }
        Err(e) => {
            log::error!("  [FAIL] IST[0]/[1] separation: {}", e);
            failed += 1;
        }
    }

    (passed, failed)
}

/// Run contracts on all process page tables
#[cfg(feature = "testing")]
pub fn test_all_processes() -> (usize, usize) {
    log::info!("Testing process page tables...");

    let mut passed = 0;
    let mut failed = 0;

    let phys_offset = crate::memory::physical_memory_offset();

    // Get master PML4 for inheritance comparison
    let master_pml4 = match crate::memory::kernel_page_table::master_kernel_pml4() {
        Some(frame) => {
            let virt = phys_offset + frame.start_address().as_u64();
            Some(unsafe { &*(virt.as_ptr() as *const PageTable) })
        }
        None => None,
    };

    // Try to get process manager (try_manager returns Option<MutexGuard<Option<ProcessManager>>>)
    if let Some(manager_guard) = crate::process::try_manager() {
        if let Some(ref manager) = *manager_guard {
            let processes = manager.all_processes();
            let process_count = processes.len();

            if process_count == 0 {
                log::info!("  [SKIP] No processes to test");
                return (passed, failed);
            }

            log::info!("  Testing {} processes...", process_count);

            for proc in processes.iter() {
                let pid = proc.pid();

                // Get process page table frame
                if let Some(page_table) = proc.page_table() {
                    let proc_pml4_frame = page_table.level_4_frame();
                    let proc_pml4_virt = phys_offset + proc_pml4_frame.start_address().as_u64();
                    let proc_pml4 = unsafe { &*(proc_pml4_virt.as_ptr() as *const PageTable) };

                    // Test frame separation
                    match page_table::verify_kernel_ist_frame_separation(proc_pml4) {
                        Ok(()) => {
                            passed += 1;
                        }
                        Err(e) => {
                            log::error!("  [FAIL] Process {} frame separation: {}", pid.as_u64(), e);
                            failed += 1;
                        }
                    }

                    // Test stack regions
                    match page_table::verify_stack_regions_present(proc_pml4) {
                        Ok(()) => {
                            passed += 1;
                        }
                        Err(e) => {
                            log::error!("  [FAIL] Process {} stack regions: {}", pid.as_u64(), e);
                            failed += 1;
                        }
                    }

                    // Test inheritance from master
                    if let Some(master) = master_pml4 {
                        match page_table::verify_kernel_mapping_inheritance(proc_pml4, master) {
                            Ok(()) => {
                                passed += 1;
                            }
                            Err(e) => {
                                log::error!("  [FAIL] Process {} inheritance: {}", pid.as_u64(), e);
                                failed += 1;
                            }
                        }
                    }
                }
            }

            if failed == 0 && process_count > 0 {
                log::info!("  [PASS] All {} processes passed contract tests", process_count);
            }
        } else {
            log::info!("  [SKIP] Process manager not initialized");
        }
    } else {
        log::info!("  [SKIP] Process manager not available (locked)");
    }

    (passed, failed)
}

/// Stub for when testing feature is disabled
#[cfg(not(feature = "testing"))]
pub fn run_all_contracts() -> (usize, usize) {
    (0, 0)
}
