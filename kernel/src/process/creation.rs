//! Proper process creation with user threads from start
//!
//! This module implements the new process creation model that follows Unix semantics:
//! - Processes are created as user threads from the beginning
//! - No kernel-to-user transitions via spawn threads
//! - Direct creation of user threads in Ring 3 mode
//! - Proper integration with the new minimal timer interrupt system

use crate::process::ProcessId;
use alloc::string::String;
use alloc::boxed::Box;

/// Create a new user process directly without spawn mechanism
/// 
/// This creates a process that starts as a userspace thread from the beginning,
/// following proper Unix semantics. The process is ready to be scheduled and
/// will start executing in Ring 3 userspace.
/// 
/// This is a thin wrapper around the existing process creation that ensures
/// the process starts as a user thread without spawn thread transitions.
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    crate::serial_println!("DEBUG: create_user_process() ENTRY");
    log::info!("create_user_process: Creating user process '{}' with optimized interrupts", name);
    
    // STEP 1: Quick PID allocation with interrupts disabled  
    crate::serial_println!("DEBUG: About to call with_process_manager");
    let pid = crate::process::with_process_manager(|manager| {
        // Just allocate the PID, don't do heavy work yet
        let pid = ProcessId::new(manager.get_next_pid());
        // NO LOGGING in interrupt-disabled section to avoid deadlock
        pid
    }).ok_or("Process manager not available")?;
    crate::serial_println!("DEBUG: PID allocation complete");
    
    log::info!("Allocated PID {} for process '{}'", pid.as_u64(), name);
    
    // STEP 2: Heavy operations with interrupts ENABLED
    // This includes page table creation, ELF loading, and stack allocation
    log::info!("Creating page table for PID {}", pid.as_u64());
    
    // Create page table
    crate::serial_println!("PCREATE: 1. page tables starting");
    let mut page_table = Box::new(
        crate::memory::process_memory::ProcessPageTable::new()
            .map_err(|e| {
                log::error!("Failed to create process page table for PID {}: {}", pid.as_u64(), e);
                "Failed to create process page table"
            })?
    );
    crate::serial_println!("PCREATE: 1. page tables ready");
    
    // Load ELF
    crate::serial_println!("PCREATE: 2. ELF loading starting");
    let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, page_table.as_mut())?;
    crate::serial_println!("PCREATE: 2. ELF loaded");
    
    // Allocate stack
    crate::serial_println!("PCREATE: 3. stack mapping starting");
    use crate::memory::stack;
    const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
    let user_stack = stack::allocate_stack(USER_STACK_SIZE)
        .map_err(|_| "Failed to allocate user stack")?;
    let stack_top = user_stack.top();
    
    // Map stack to process page table
    let stack_bottom = stack_top - USER_STACK_SIZE as u64;
    crate::memory::process_memory::map_user_stack_to_process(&mut page_table, stack_bottom, stack_top)
        .map_err(|e| {
            log::error!("Failed to map user stack to process page table: {}", e);
            "Failed to map user stack in process page table"
        })?;
    crate::serial_println!("PCREATE: 3. stack mapped");
    
    log::info!("Heavy operations completed for PID {} (page table, ELF, stack)", pid.as_u64());
    
    // STEP 3: Final bookkeeping with interrupts disabled (quick operations only)
    let thread_id = crate::process::with_process_manager(|manager| {
        // Create process structure
        let mut process = crate::process::Process::new(pid, name.clone(), loaded_elf.entry_point);
        process.page_table = Some(page_table);
        process.memory_usage.code_size = elf_data.len();
        process.memory_usage.stack_size = USER_STACK_SIZE;
        process.stack = Some(Box::new(user_stack));
        
        // Create main thread
        crate::serial_println!("PCREATE: 4. thread object building");
        let thread = manager.create_main_thread_for_process(&mut process, stack_top)?;
        let thread_id = thread.id;
        let thread_privilege = thread.privilege;
        process.set_main_thread(thread.clone());
        crate::serial_println!("PCREATE: 4. thread object built");
        
        // Add to process table and ready queue
        manager.add_process_to_tables(pid, process)?;
        
        // Add thread to scheduler
        crate::serial_println!("PCREATE: 5. scheduler::push about to call");
        if thread_privilege == crate::task::thread::ThreadPrivilege::User {
            crate::task::scheduler::spawn(Box::new(thread));
            crate::serial_println!("PCREATE: 5. scheduler::push completed");
            Ok(thread_id)
        } else {
            Err("Created thread is not a user thread")
        }
    }).ok_or("Process manager not available")??;
    crate::serial_println!("STEP2-C DONE");
    
    log::info!("create_user_process: Added user thread {} directly to scheduler", thread_id);
    
    log::info!("create_user_process: Successfully created user process {} with optimized interrupts", 
               pid.as_u64());
    
    Ok(pid)
}

