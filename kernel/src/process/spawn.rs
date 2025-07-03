//! Process spawning mechanism
//!
//! This module implements proper process spawning using dedicated kernel threads.
//! Unlike the broken timer interrupt approach, this ensures processes start with
//! correct register state and avoids race conditions.

use crate::process::{ProcessId, manager};
use crate::task::{thread::Thread, thread::ThreadPrivilege, scheduler};
use alloc::string::String;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::VirtAddr;

/// Spawn request for the spawn thread
struct SpawnRequest {
    name: String,
    elf_data: Vec<u8>,
}

/// Global spawn queue
static SPAWN_QUEUE: Mutex<Vec<SpawnRequest>> = Mutex::new(Vec::new());

/// Spawn a process in the background
/// 
/// This queues a spawn request and creates a kernel thread to handle it.
/// The kernel thread will call exec_process() to transition to userspace.
pub fn spawn_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("spawn_process: Spawning process {}", name);
    
    // First, create the process to get its PID
    let pid = {
        let mut manager_guard = manager();
        let manager = manager_guard.as_mut()
            .ok_or("Process manager not initialized")?;
        
        // Create the process but don't schedule it yet
        manager.create_process(name.clone(), elf_data)?
    };
    
    log::info!("spawn_process: Created process {} with PID {}", name, pid.as_u64());
    
    // Create a kernel thread that will exec into this process
    let spawn_thread_name = alloc::format!("spawn_{}", pid.as_u64());
    
    // Create the spawn thread
    let spawn_thread = Box::new(Thread::new_kernel(
        spawn_thread_name,
        spawn_thread_entry,
        pid.as_u64(), // Pass PID as argument
    )?);
    
    // Add the spawn thread to the scheduler
    scheduler::spawn(spawn_thread);
    
    log::info!("spawn_process: Created spawn thread for PID {}", pid.as_u64());
    
    Ok(pid)
}

/// Entry point for spawn threads
/// 
/// This function runs in a kernel thread and calls exec to transition
/// to the userspace process. It never returns.
extern "C" fn spawn_thread_entry(pid_arg: u64) -> ! {
    let pid = ProcessId::new(pid_arg);
    log::info!("spawn_thread_entry: Starting spawn thread for PID {}", pid.as_u64());
    log::info!("spawn_thread_entry: This is the spawn thread executing!");
    
    // Get the process info we need
    let (name, entry_point, stack_top) = {
        let mut manager_guard = manager();
        let manager = match manager_guard.as_mut() {
            Some(m) => m,
            None => {
                log::error!("spawn_thread_entry: Process manager not initialized");
                loop { core::hint::spin_loop(); }
            }
        };
        
        let process = match manager.get_process(pid) {
            Some(p) => p,
            None => {
                log::error!("spawn_thread_entry: Process {} not found", pid.as_u64());
                loop { core::hint::spin_loop(); }
            }
        };
        
        let thread = match process.main_thread.as_ref() {
            Some(t) => t,
            None => {
                log::error!("spawn_thread_entry: Process {} has no main thread", pid.as_u64());
                loop { core::hint::spin_loop(); }
            }
        };
        
        (
            process.name.clone(),
            thread.context.rip,
            thread.context.rsp,
        )
    };
    
    log::info!("spawn_thread_entry: Preparing to exec {} (entry={:#x}, stack={:#x})", 
              name, entry_point, stack_top);
    
    // Set up TLS for the target thread
    if let Err(e) = crate::tls::switch_tls(pid.as_u64()) {
        log::error!("spawn_thread_entry: Failed to set up TLS: {}", e);
        loop { core::hint::spin_loop(); }
    }
    
    // Update TSS RSP0 for syscalls
    // First get the thread's kernel stack
    let kernel_stack_top = {
        let manager_guard = manager();
        let manager = manager_guard.as_ref().unwrap();
        let process = manager.get_process(pid).unwrap();
        let thread = process.main_thread.as_ref().unwrap();
        thread.stack_top
    };
    
    crate::gdt::set_kernel_stack(kernel_stack_top);
    
    log::info!("spawn_thread_entry: Switching to userspace for PID {}", pid.as_u64());
    
    // IMPORTANT: We need to be careful about scheduler locks here
    // The spawn thread is about to transform into the userspace process
    
    // First, get the thread we need to schedule
    let thread_to_schedule = {
        let mut manager_guard = manager();
        let manager = manager_guard.as_mut().unwrap();
        let process = manager.get_process(pid).unwrap();
        process.main_thread.as_ref().unwrap().clone()
    };
    
    // Now do all scheduler operations in one go to avoid deadlock
    scheduler::with_scheduler(|sched| {
        // Add the userspace thread
        sched.add_thread(Box::new(thread_to_schedule));
        log::info!("spawn_thread_entry: Added userspace thread {} to scheduler", pid.as_u64());
        
        // Mark the spawn thread as terminated since it's about to disappear
        if let Some(spawn_thread) = sched.get_thread_mut(scheduler::current_thread_id().unwrap()) {
            spawn_thread.set_terminated();
        }
        
        // Update the current thread to be the userspace thread
        sched.set_current_thread(pid.as_u64());
        
        log::info!("spawn_thread_entry: Updated scheduler - current thread is now {}", pid.as_u64());
    });
    
    // Clear all general purpose registers for security
    unsafe {
        // Set up segments for Ring 3
        let user_cs = crate::gdt::USER_CODE_SELECTOR.0 | 3;  // Set RPL=3
        let user_ds = crate::gdt::USER_DATA_SELECTOR.0 | 3;  // Set RPL=3
        core::arch::asm!(
            "xor rax, rax",
            "xor rbx, rbx", 
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor rsi, rsi",
            "xor rdi, rdi",
            "xor rbp, rbp",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",
            options(nomem, nostack)
        );
        
        // Switch to userspace - this never returns
        crate::task::userspace_switch::switch_to_userspace(
            VirtAddr::new(entry_point),
            VirtAddr::new(stack_top),
            user_cs as u16,
            user_ds as u16,
        )
    }
}

/// Spawn the init process (PID 1)
/// 
/// This is called during kernel initialization to start the first process.
pub fn spawn_init(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("Starting init process");
    spawn_process(name, elf_data)
}