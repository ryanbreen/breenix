//! Process manager - handles process lifecycle and scheduling

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use core::sync::atomic::{self, AtomicU64, Ordering};
use x86_64::VirtAddr;

use super::{Process, ProcessId};
use crate::task::thread::Thread;
use crate::memory::process_memory::ProcessPageTable;

/// Process manager handles all processes in the system
pub struct ProcessManager {
    /// All processes indexed by PID
    processes: BTreeMap<ProcessId, Process>,
    
    
    /// Next available PID
    next_pid: AtomicU64,
    
    /// Queue of ready processes
    ready_queue: Vec<ProcessId>,
    
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        ProcessManager {
            processes: BTreeMap::new(),
            next_pid: AtomicU64::new(1), // PIDs start at 1 (0 is kernel)
            ready_queue: Vec::new(),
        }
    }
    
    /// Get the next available PID without creating a process
    pub fn get_next_pid(&mut self) -> u64 {
        self.next_pid.fetch_add(1, Ordering::SeqCst)
    }
    
    /// Create main thread for a process (public helper for optimized creation)
    pub fn create_main_thread_for_process(&mut self, process: &mut Process, stack_top: VirtAddr) -> Result<Thread, &'static str> {
        self.create_main_thread(process, stack_top)
    }
    
    /// Add a fully constructed process to the process table and ready queue
    pub fn add_process_to_tables(&mut self, pid: ProcessId, process: Process) -> Result<(), &'static str> {
        // Add to ready queue
        self.ready_queue.push(pid);
        
        // Insert into process table
        self.processes.insert(pid, process);
        
        log::info!("Added process {} to tables", pid.as_u64());
        Ok(())
    }
    
    /// Create a new process from an ELF binary
    pub fn create_process(&mut self, name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
        // Generate a new PID
        let pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        
        // Create a new page table for this process
        let mut page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|e| {
                    log::error!("Failed to create process page table for PID {}: {}", pid.as_u64(), e);
                    "Failed to create process page table"
                })?
        );
        
        // WORKAROUND: We'd like to clear existing userspace mappings before loading ELF
        // but since L3 tables are shared between processes, unmapping pages affects
        // all processes sharing that table. This causes double faults.
        // For now, we'll skip this and let the ELF loader fail on "page already mapped"
        // errors for the second process.
        /*
        page_table.clear_userspace_for_exec()
            .map_err(|e| {
                log::error!("Failed to clear userspace mappings: {}", e);
                "Failed to clear userspace mappings"
            })?;
        */
        
        // Load the ELF binary into the process's page table
        // Use the standard userspace base address for all processes
        let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, page_table.as_mut())?;
        
        // Create the process
        let mut process = Process::new(pid, name.clone(), loaded_elf.entry_point);
        process.page_table = Some(page_table);
        
        // Update memory usage
        process.memory_usage.code_size = elf_data.len();
        
        // Allocate a stack for the process
        use crate::memory::stack;
        
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        let user_stack = stack::allocate_stack(USER_STACK_SIZE).map_err(|_| "Failed to allocate user stack")?;
        
        let stack_top = user_stack.top();
        process.memory_usage.stack_size = USER_STACK_SIZE;
        
        // Store the stack in the process
        process.stack = Some(Box::new(user_stack));
        
        // CRITICAL: Map the user stack pages into the process page table
        // The stack was allocated in the kernel page table, but userspace needs it mapped
        if let Some(ref mut page_table) = process.page_table {
            let stack_bottom = stack_top - USER_STACK_SIZE as u64;
            crate::memory::process_memory::map_user_stack_to_process(page_table, stack_bottom, stack_top)
                .map_err(|e| {
                    log::error!("Failed to map user stack to process page table: {}", e);
                    "Failed to map user stack in process page table"
                })?;
        } else {
            return Err("Process page table not available for stack mapping");
        }
        
        // Create the main thread
        let thread = self.create_main_thread(&mut process, stack_top)?;
        process.set_main_thread(thread);
        
        // Add to ready queue
        self.ready_queue.push(pid);
        
        // Insert into process table
        self.processes.insert(pid, process);
        
        log::info!("Created process {} (PID {})", name, pid.as_u64());
        
        Ok(pid)
    }
    
    /// Create the main thread for a process
    fn create_main_thread(&mut self, process: &mut Process, stack_top: x86_64::VirtAddr) -> Result<Thread, &'static str> {
        
        // For now, use a null TLS block (we'll implement TLS later)
        let _tls_block = x86_64::VirtAddr::new(0);
        
        // For the main thread, use PID as TID (Unix convention)
        let thread_id = process.id.as_u64();
        
        // Allocate a TLS block for this thread ID
        let actual_tls_block = VirtAddr::new(0x10000 + thread_id * 0x1000);
        
        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(thread_id, actual_tls_block) {
            log::warn!("Failed to register thread {} with TLS system: {}", thread_id, e);
        }
        
        // Calculate stack bottom (stack grows down)
        const USER_STACK_SIZE: usize = 64 * 1024;
        let stack_bottom = stack_top - USER_STACK_SIZE as u64;
        
        // Allocate a kernel stack using the new global kernel stack allocator
        // This automatically maps the stack in the global kernel page tables,
        // making it visible to all processes
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack()
            .map_err(|e| {
                log::error!("Failed to allocate kernel stack: {}", e);
                "Failed to allocate kernel stack for thread"
            })?;
        let kernel_stack_top = kernel_stack.top();
        
        
        // Store the kernel stack - it will be dropped when the thread is destroyed
        // For now, we'll leak it - TODO: proper cleanup
        Box::leak(Box::new(kernel_stack));
        
        // Set up initial context for userspace
        let context = crate::task::thread::CpuContext::new(
            process.entry_point,
            stack_top,
            crate::task::thread::ThreadPrivilege::User,
        );
        
        let thread = Thread {
            id: thread_id,
            name: String::from(&process.name),
            state: crate::task::thread::ThreadState::Ready,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top: Some(kernel_stack_top),
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: crate::task::thread::ThreadPrivilege::User,
            first_run: false,  // New thread hasn't run yet
            ticks_run: 0,      // No timer ticks yet
        };
        
        Ok(thread)
    }
    
    
    /// Get a reference to a process
    pub fn get_process(&self, pid: ProcessId) -> Option<&Process> {
        self.processes.get(&pid)
    }
    
    /// Get a mutable reference to a process
    pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
        self.processes.get_mut(&pid)
    }
    
    /// Remove a process from the ready queue
    pub fn remove_from_ready_queue(&mut self, pid: ProcessId) -> bool {
        if let Some(index) = self.ready_queue.iter().position(|&p| p == pid) {
            self.ready_queue.remove(index);
            true
        } else {
            false
        }
    }
    
    /// Add a process to the ready queue
    pub fn add_to_ready_queue(&mut self, pid: ProcessId) {
        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
        }
    }
    
    /// Find a process by its main thread ID
    pub fn find_process_by_thread(&self, thread_id: u64) -> Option<(ProcessId, &Process)> {
        self.processes.iter()
            .find(|(_, process)| process.main_thread.as_ref().map(|t| t.id) == Some(thread_id))
            .map(|(pid, process)| (*pid, process))
    }
    
    /// Find a process by its main thread ID (mutable)
    pub fn find_process_by_thread_mut(&mut self, thread_id: u64) -> Option<(ProcessId, &mut Process)> {
        self.processes.iter_mut()
            .find(|(_, process)| process.main_thread.as_ref().map(|t| t.id) == Some(thread_id))
            .map(|(pid, process)| (*pid, process))
    }
    
    /// Debug print all processes
    pub fn debug_processes(&self) {
        log::info!("=== Process List ===");
        for (pid, process) in &self.processes {
            log::info!("  PID {}: {} - {:?}", 
                pid.as_u64(), 
                process.name, 
                process.state
            );
        }
        log::info!("Ready queue: {:?}", self.ready_queue);
    }
    
    /// Fork a process - create a child process that's a copy of the parent
    pub fn fork_process(&mut self, parent_pid: ProcessId) -> Result<ProcessId, &'static str> {
        self.fork_process_with_context(parent_pid, None)
    }
    
    /// Fork a process with full syscall frame context
    /// This version uses the actual syscall context instead of stale saved context
    pub fn fork_process_with_frame(&mut self, parent_pid: ProcessId, 
                                   frame: &crate::syscall::handler::SyscallFrame,
                                   mut child_page_table: Box<ProcessPageTable>) -> Result<ProcessId, &'static str> {
        // Get the parent process info we need
        let (parent_name, parent_entry_point) = {
            let parent = self.processes.get(&parent_pid)
                .ok_or("Parent process not found")?;
            
            // Clone what we need to avoid borrow issues
            (parent.name.clone(), parent.entry_point)
        };
        
        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, atomic::Ordering::SeqCst));
        
        log::info!("Forking process {} '{}' -> child PID {} with syscall frame", 
            parent_pid.as_u64(), parent_name, child_pid.as_u64());
        
        // Create child process name
        let child_name = format!("{}_child_{}", parent_name, child_pid.as_u64());
        
        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent_entry_point);
        child_process.parent = Some(parent_pid);
        
        // Get parent page table for memory cloning
        let parent_page_table = self.processes.get(&parent_pid)
            .ok_or("Parent process not found")?
            .page_table.as_ref()
            .ok_or("Parent process has no page table")?;
        
        // Clone parent's memory to child using fork helpers
        log::info!("fork_process: Cloning parent memory to child");
        let (copied_pages, shared_pages) = crate::memory::fork_helpers::clone_process_memory(
            parent_page_table,
            child_page_table.as_mut(),
            parent_pid.as_u64(),
            child_pid.as_u64(),
        )?;
        
        log::info!("fork_process: Memory clone complete - {} pages copied, {} pages shared",
                  copied_pages, shared_pages);
        
        // Store the page table in the child process
        child_process.page_table = Some(child_page_table);
        
        // Complete the fork with the syscall frame context
        self.complete_fork_with_frame(parent_pid, child_pid, frame, child_process)
    }

    /// Fork a process with a pre-allocated page table (OLD VERSION - uses stale context)
    /// This version accepts a page table created outside the lock to avoid deadlock
    #[allow(unused_variables, unused_mut)]
    pub fn fork_process_with_page_table(&mut self, parent_pid: ProcessId, userspace_rsp: Option<u64>, 
                                       mut child_page_table: Box<ProcessPageTable>) -> Result<ProcessId, &'static str> {
        // Get the parent process info we need
        let (parent_name, parent_entry_point, _parent_thread_info) = {
            let parent = self.processes.get(&parent_pid)
                .ok_or("Parent process not found")?;
            
            let parent_thread = parent.main_thread.as_ref()
                .ok_or("Parent process has no main thread")?;
            
            // Clone what we need to avoid borrow issues
            (parent.name.clone(), 
             parent.entry_point,
             parent_thread.clone())
        };
        
        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, atomic::Ordering::SeqCst));
        
        log::info!("Forking process {} '{}' -> child PID {}", 
            parent_pid.as_u64(), parent_name, child_pid.as_u64());
        
        // Create child process name
        let child_name = format!("{}_child_{}", parent_name, child_pid.as_u64());
        
        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent_entry_point);
        child_process.parent = Some(parent_pid);
        
        // Get parent page table for memory cloning
        let parent_page_table = self.processes.get(&parent_pid)
            .ok_or("Parent process not found")?
            .page_table.as_ref()
            .ok_or("Parent process has no page table")?;
        
        // Clone parent's memory to child using fork helpers
        log::info!("fork_process: Cloning parent memory to child");
        let (copied_pages, shared_pages) = crate::memory::fork_helpers::clone_process_memory(
            parent_page_table,
            child_page_table.as_mut(),
            parent_pid.as_u64(),
            child_pid.as_u64(),
        )?;
        
        log::info!("fork_process: Memory clone complete - {} pages copied, {} pages shared",
                  copied_pages, shared_pages);
        
        // Store the page table in the child process
        child_process.page_table = Some(child_page_table);
        
        // Continue with the rest of the fork logic...
        self.complete_fork(parent_pid, child_pid, &_parent_thread_info, userspace_rsp, child_process)
    }
    
    /// Complete the fork operation with syscall frame context
    fn complete_fork_with_frame(&mut self, parent_pid: ProcessId, child_pid: ProcessId, 
                               frame: &crate::syscall::handler::SyscallFrame,
                               mut child_process: Process) -> Result<ProcessId, &'static str> {
        log::info!("Completing fork with syscall frame context");
        
        // Get parent thread for stack bounds
        let parent_thread = self.processes.get(&parent_pid)
            .ok_or("Parent process not found")?
            .main_thread.as_ref()
            .ok_or("Parent process has no main thread")?;
        
        // CRITICAL: Child uses same stack addresses as parent (already cloned)
        let child_stack_top = parent_thread.stack_top;
        let child_stack_bottom = parent_thread.stack_bottom;
        
        // Create the child thread ID (same as PID for main thread)
        let child_thread_id = child_pid.as_u64();
        
        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);
        
        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
            log::warn!("Failed to register thread {} with TLS system: {}", child_thread_id, e);
        }
        
        // Allocate a kernel stack for the child thread
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack()
            .map_err(|e| {
                log::error!("Failed to allocate kernel stack for child: {}", e);
                "Failed to allocate kernel stack for child thread"
            })?;
        let child_kernel_stack_top = kernel_stack.top();
        
        // Store the kernel stack (we'll need to manage this properly later)
        Box::leak(Box::new(kernel_stack));
        
        // Create the child thread with context from syscall frame
        let mut child_thread = Thread::new(
            format!("{}_main", child_process.name),
            || {}, // dummy entry - we use context
            child_stack_top,
            child_stack_bottom,
            child_tls_block,
            crate::task::thread::ThreadPrivilege::User,
        );
        
        // Set the ID and kernel stack
        child_thread.id = child_thread_id;
        child_thread.kernel_stack_top = Some(child_kernel_stack_top);
        
        // CRITICAL: Copy context from syscall frame, not stale saved context
        log::info!("Fork: Copying syscall frame to child context");
        log::info!("  Frame RIP: {:#x}, RSP: {:#x}, RAX: {:#x}", frame.rip, frame.rsp, frame.rax);
        
        // Copy all registers from syscall frame
        child_thread.context.rax = 0;  // Fork returns 0 in child
        child_thread.context.rbx = frame.rbx;
        child_thread.context.rcx = frame.rcx;
        child_thread.context.rdx = frame.rdx;
        child_thread.context.rsi = frame.rsi;
        child_thread.context.rdi = frame.rdi;
        child_thread.context.rbp = frame.rbp;
        child_thread.context.rsp = frame.rsp;
        child_thread.context.r8 = frame.r8;
        child_thread.context.r9 = frame.r9;
        child_thread.context.r10 = frame.r10;
        child_thread.context.r11 = frame.r11;
        child_thread.context.r12 = frame.r12;
        child_thread.context.r13 = frame.r13;
        child_thread.context.r14 = frame.r14;
        child_thread.context.r15 = frame.r15;
        child_thread.context.rip = frame.rip;
        child_thread.context.cs = frame.cs;
        child_thread.context.ss = frame.ss;
        child_thread.context.rflags = frame.rflags;
        
        
        log::info!("Fork: Child context set - RIP: {:#x}, RSP: {:#x}, RAX: 0", 
                  child_thread.context.rip, child_thread.context.rsp);
        
        // Set the child process's main thread
        child_process.main_thread = Some(child_thread);
        
        // Add the child to the parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.children.push(child_pid);
        }
        
        // Insert the child process into the process table
        self.processes.insert(child_pid, child_process);
        
        log::info!("Fork complete: parent {} -> child {}", parent_pid.as_u64(), child_pid.as_u64());
        
        // DEBUG: Keep timer interrupts enabled since they're needed for scheduling
        // Previous test showed disabling timer prevents child from being scheduled at all
        
        // DEBUG: Skip INT3 installation since code pages are read-only
        // The trap flag should be sufficient for debugging
        
        Ok(child_pid)
    }

    /// Complete the fork operation after page table is created (OLD VERSION)
    #[allow(dead_code)]
    fn complete_fork(&mut self, parent_pid: ProcessId, child_pid: ProcessId, 
                     parent_thread: &Thread, userspace_rsp: Option<u64>, 
                     mut child_process: Process) -> Result<ProcessId, &'static str> {
        log::info!("Created page table for child process {}", child_pid.as_u64());
        
        // CRITICAL FORK FIX: The child uses the SAME stack addresses as parent
        // The stack pages are already cloned by clone_process_memory
        // We just need to use the parent's stack bounds
        let child_stack_top = parent_thread.stack_top;
        let _child_stack_bottom = parent_thread.stack_bottom;
        
        // For now, use a dummy TLS address - the Thread constructor will allocate proper TLS
        // In the future, we should properly copy parent's TLS data
        let _dummy_tls = VirtAddr::new(0);
        
        // Create the child thread with PID as TID (Unix convention for main thread)
        let child_thread_id = child_pid.as_u64();
        
        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);
        
        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
            log::warn!("Failed to register thread {} with TLS system: {}", child_thread_id, e);
        }
        
        // Allocate a kernel stack for the child thread (userspace threads need kernel stacks)
        let child_kernel_stack_top = if parent_thread.privilege == crate::task::thread::ThreadPrivilege::User {
            // Use the new global kernel stack allocator
            let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack()
                .map_err(|e| {
                    log::error!("Failed to allocate kernel stack for child: {}", e);
                    "Failed to allocate kernel stack for child thread"
                })?;
            let kernel_stack_top = kernel_stack.top();
            
            
            // Store the kernel stack (we'll need to manage this properly later)
            // For now, we'll leak it - TODO: proper cleanup
            Box::leak(Box::new(kernel_stack));
            
            kernel_stack_top
        } else {
            // Kernel threads don't need separate kernel stacks
            parent_thread.kernel_stack_top.unwrap_or(parent_thread.stack_top)
        };
        
        // Create the child's main thread
        // The child thread starts with the same state as the parent, but with:
        // - New thread ID (same as PID for main thread)
        // - RSP pointing to the new stack
        // - RDI set to 0 (to indicate child process in fork return)
        // Create a dummy entry function - we'll set the real entry point via context
        fn dummy_entry() {}
        
        let mut child_thread = Thread::new(
            format!("{}_main", child_process.name),
            dummy_entry,
            child_stack_top,
            parent_thread.stack_bottom,
            child_tls_block,
            parent_thread.privilege,
        );
        
        // Set the ID and kernel stack separately
        child_thread.id = child_thread_id;
        child_thread.kernel_stack_top = Some(child_kernel_stack_top);
        
        // Copy parent's thread context
        child_thread.context = parent_thread.context.clone();
        
        // Log the child's context for debugging
        log::info!("Fork: Parent context RIP: {:#x}, RSP: {:#x}, RAX: {:#x}", 
                 parent_thread.context.rip, parent_thread.context.rsp, parent_thread.context.rax);
        log::info!("Fork: Child context after copy - RIP: {:#x}, RSP: {:#x}", 
                 child_thread.context.rip, child_thread.context.rsp);
        log::debug!("  CS: {:#x}", child_thread.context.cs);
        log::debug!("  SS: {:#x}", child_thread.context.ss);
        
        // Crucial: Set the child's return value to 0
        // In x86_64, system call return values go in RAX
        child_thread.context.rax = 0;
        log::info!("Fork: Child RAX set to 0 (fork return value)");
        
        // CRITICAL: Child uses SAME RSP as parent (fork semantics)
        // The userspace_rsp is the parent's RSP at time of fork
        if let Some(user_rsp) = userspace_rsp {
            child_thread.context.rsp = user_rsp;
            log::info!("fork: Child uses parent's RSP {:#x}", user_rsp);
        } else {
            // This shouldn't happen - we should always have the parent's RSP
            log::error!("fork: No userspace RSP provided!");
            child_thread.context.rsp = parent_thread.context.rsp;
        }
        
        log::info!("Created child thread {} with entry point {:#x}", 
                  child_thread_id, child_process.entry_point);
        
        // Set the child process's main thread
        child_process.main_thread = Some(child_thread);
        
        // NOTE: Stack is not stored because child uses parent's stack addresses (cloned pages)
        
        // Add the child to the parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.children.push(child_pid);
        }
        
        // Insert the child process into the process table
        log::debug!("About to insert child process into process table");
        self.processes.insert(child_pid, child_process);
        log::debug!("Child process inserted successfully");
        
        log::info!("Fork complete: parent {} -> child {}", parent_pid.as_u64(), child_pid.as_u64());
        
        Ok(child_pid)
    }
    
    /// Fork a process with optional userspace context override
    /// NOTE: This method creates the page table while holding the lock, which can cause deadlock
    /// Consider using fork_process_with_page_table instead
    #[allow(unused_variables, unused_mut)]
    pub fn fork_process_with_context(&mut self, parent_pid: ProcessId, userspace_rsp: Option<u64>) -> Result<ProcessId, &'static str> {
        // Get the parent process
        let parent = self.processes.get(&parent_pid)
            .ok_or("Parent process not found")?;
        
        // Get parent's main thread
        let parent_thread = parent.main_thread.as_ref()
            .ok_or("Parent process has no main thread")?;
        
        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, atomic::Ordering::SeqCst));
        
        log::info!("Forking process {} '{}' -> child PID {}", 
            parent_pid.as_u64(), parent.name, child_pid.as_u64());
        
        // Create child process name
        let child_name = format!("{}_child_{}", parent.name, child_pid.as_u64());
        
        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent.entry_point);
        child_process.parent = Some(parent_pid);
        
        // Create a new page table for the child process
        let parent_page_table = parent.page_table.as_ref()
            .ok_or("Parent process has no page table")?;
        
        // DEBUG: Test parent page table before creating child
        log::debug!("BEFORE creating child page table:");
        let test_addr = VirtAddr::new(0x10001000);
        match parent_page_table.translate_page(test_addr) {
            Some(phys) => log::debug!("Parent can translate {:#x} -> {:#x}", test_addr, phys),
            None => log::debug!("Parent CANNOT translate {:#x}", test_addr),
        }
        
        // Create a new page table and copy parent's program memory
        log::debug!("fork_process: About to create child page table");
        let child_page_table_result = crate::memory::process_memory::ProcessPageTable::new();
        log::debug!("fork_process: ProcessPageTable::new() returned");
        let mut child_page_table = Box::new(
            child_page_table_result
                .map_err(|_| "Failed to create child page table")?
        );
        log::debug!("fork_process: Child page table created successfully");
        
        // DEBUG: Test parent page table after creating child
        log::debug!("AFTER creating child page table:");
        match parent_page_table.translate_page(test_addr) {
            Some(phys) => log::debug!("Parent can translate {:#x} -> {:#x}", test_addr, phys),
            None => log::debug!("Parent CANNOT translate {:#x}", test_addr),
        }
        
        // Log page table addresses for debugging
        log::debug!("Parent page table CR3: {:#x}", parent_page_table.level_4_frame().start_address());
        log::debug!("Child page table CR3: {:#x}", child_page_table.level_4_frame().start_address());
        
        // Clone parent's memory to child using fork helpers
        log::info!("fork_process: Cloning parent memory to child");
        let (copied_pages, shared_pages) = crate::memory::fork_helpers::clone_process_memory(
            parent_page_table,
            child_page_table.as_mut(),
            parent_pid.as_u64(),
            child_pid.as_u64(),
        )?;
        
        log::info!("fork_process: Memory clone complete - {} pages copied, {} pages shared",
                  copied_pages, shared_pages);
        log::info!("Created page table for child process {}", child_pid.as_u64());
        
        // CRITICAL FORK FIX: The child uses the SAME stack addresses as parent
        // The stack pages are already cloned by clone_process_memory
        let child_stack_top = parent_thread.stack_top;
        
        // For now, use a dummy TLS address - the Thread constructor will allocate proper TLS
        // In the future, we should properly copy parent's TLS data
        let _dummy_tls = VirtAddr::new(0);
        
        // Create the child thread with PID as TID (Unix convention for main thread)
        let child_thread_id = child_pid.as_u64();
        
        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);
        
        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
            log::warn!("Failed to register thread {} with TLS system: {}", child_thread_id, e);
        }
        
        // Allocate a kernel stack for the child thread (userspace threads need kernel stacks)
        let child_kernel_stack_top = if parent_thread.privilege == crate::task::thread::ThreadPrivilege::User {
            const KERNEL_STACK_SIZE: usize = 16 * 1024; // 16KB kernel stack
            let kernel_stack = crate::memory::stack::allocate_stack(KERNEL_STACK_SIZE)
                .map_err(|_| "Failed to allocate kernel stack for child thread")?;
            let kernel_stack_top = kernel_stack.top();
            
            // Store kernel_stack data for later use
            let _kernel_stack_bottom = kernel_stack.bottom();
            
            // Store the kernel stack (we'll need to manage this properly later)
            // For now, we'll leak it - TODO: proper cleanup
            Box::leak(Box::new(kernel_stack));
            
            Some(kernel_stack_top)
        } else {
            None
        };
        
        // Create the child thread manually to use specific ID
        let mut child_thread = Thread {
            id: child_thread_id,
            name: child_name,
            state: crate::task::thread::ThreadState::Ready,
            context: parent_thread.context.clone(), // Will be modified below
            stack_top: parent_thread.stack_top,
            stack_bottom: parent_thread.stack_bottom,
            kernel_stack_top: child_kernel_stack_top,
            tls_block: child_tls_block,
            priority: parent_thread.priority,
            time_slice: parent_thread.time_slice,
            entry_point: None, // Userspace threads don't have kernel entry points
            privilege: parent_thread.privilege,
            first_run: false,  // New child thread hasn't run yet
            ticks_run: 0,      // No timer ticks yet
        };
        
        // CRITICAL: Child uses SAME RSP as parent (fork semantics)
        // The userspace_rsp is the parent's RSP at time of fork
        if let Some(user_rsp) = userspace_rsp {
            child_thread.context.rsp = user_rsp;
            log::info!("fork: Child uses parent's RSP {:#x}", user_rsp);
        } else {
            // This shouldn't happen - we should always have the parent's RSP
            log::error!("fork: No userspace RSP provided!");
            child_thread.context.rsp = parent_thread.context.rsp;
        }
        
        // IMPORTANT: Set RAX to 0 for the child (fork return value)
        child_thread.context.rax = 0;
        
        // Set up child thread properties
        child_thread.privilege = parent_thread.privilege;
        // Mark child as ready to run
        child_thread.state = crate::task::thread::ThreadState::Ready;
        
        // NOTE: Stack is not stored because child uses parent's stack addresses (cloned pages)
        
        // Copy process memory from parent to child (this modifies child_thread)
        super::fork::copy_process_memory(parent_pid, &mut child_process, parent_thread, &mut child_thread)?;
        super::fork::copy_process_state(parent, &mut child_process)?;

        // Set the child thread as the main thread of the child process
        child_process.set_main_thread(child_thread);
        
        // Add child to parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.add_child(child_pid);
        }
        
        // Add the child process to the process table
        self.processes.insert(child_pid, child_process);
        
        // With global kernel page tables, all kernel stacks are automatically visible
        // to all processes through the shared kernel PDPT - no copying needed!
        if let Some(_kernel_stack_top) = child_kernel_stack_top {
        }
        
        // Add the child to the ready queue so it can be scheduled
        self.ready_queue.push(child_pid);
        
        log::info!("Fork complete: parent {} -> child {}", 
            parent_pid.as_u64(), child_pid.as_u64());
        
        // Return the child PID to the parent
        Ok(child_pid)
    }

    /// Replace a process's address space with a new program (exec)
    /// 
    /// This implements the exec() family of system calls. Unlike fork(), which creates
    /// a new process, exec() replaces the current process's address space with a new
    /// program while keeping the same PID.
    pub fn exec_process(&mut self, pid: ProcessId, elf_data: &[u8]) -> Result<u64, &'static str> {
        log::info!("exec_process: Replacing process {} with new program", pid.as_u64());
        
        // CRITICAL OS-STANDARD CHECK: Is this the current process?
        // For now, we'll assume it's not the current process since we don't track that anymore
        let is_current_process = false;
        if is_current_process {
            log::info!("exec_process: Executing on current process - special handling required");
        }
        
        // Get the existing process
        let process = self.processes.get_mut(&pid)
            .ok_or("Process not found")?;
        
        // For now, assume non-current processes are not actively running
        // This is a simplification - in a real OS we'd check the scheduler state
        let is_scheduled = false;
        
        // Get the main thread (we need to preserve its ID)
        let main_thread = process.main_thread.as_ref()
            .ok_or("Process has no main thread")?;
        let thread_id = main_thread.id;
        let _old_stack_top = main_thread.stack_top;
        
        // Store old page table for proper cleanup
        let old_page_table = process.page_table.take();
        
        log::info!("exec_process: Preserving thread ID {} for process {}", thread_id, pid.as_u64());
        
        // Load the new ELF program properly
        log::info!("exec_process: Loading new ELF program ({} bytes)", elf_data.len());
        
        // Create a new page table for the new program
        log::info!("exec_process: Creating new page table...");
        let mut new_page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|_| "Failed to create new page table for exec")?
        );
        log::info!("exec_process: New page table created successfully");
        
        // Clear any user mappings that might have been copied from the current page table
        // This prevents conflicts when loading the new program
        new_page_table.clear_user_entries();
        
        // Unmap the old program's pages in common userspace ranges
        // This is necessary because entry 0 contains both kernel and user mappings
        // Typical userspace code location: 0x10000000 - 0x10100000 (1MB range)
        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(0x10000000), 
            VirtAddr::new(0x10100000)
        ) {
            log::warn!("Failed to unmap old user code pages: {}", e);
        }
        
        // Also unmap any pages in the BSS/data area (just after code)
        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(0x10001000), 
            VirtAddr::new(0x10010000)
        ) {
            log::warn!("Failed to unmap old user data pages: {}", e);
        }
        
        log::info!("exec_process: Cleared potential user mappings from new page table");
        
        // Load the ELF binary into the new page table
        log::info!("exec_process: Loading ELF into new page table...");
        let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, new_page_table.as_mut())?;
        let new_entry_point = loaded_elf.entry_point.as_u64();
        log::info!("exec_process: ELF loaded successfully, entry point: {:#x}", new_entry_point);
        
        // CRITICAL FIX: Allocate and map stack directly into the new process page table
        // We need to manually allocate stack pages and map them into the new page table,
        // not the current kernel page table
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        const USER_STACK_TOP: u64 = 0x5555_5555_5000;
        
        // Calculate stack range
        let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
        let stack_top = VirtAddr::new(USER_STACK_TOP);
        let _guard_page = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64 - 0x1000);
        
        // Map stack pages into the NEW process page table
        log::info!("exec_process: Mapping stack pages into new process page table");
        let start_page = x86_64::structures::paging::Page::<x86_64::structures::paging::Size4KiB>::containing_address(stack_bottom);
        let end_page = x86_64::structures::paging::Page::<x86_64::structures::paging::Size4KiB>::containing_address(stack_top - 1u64);
        log::info!("exec_process: Stack range: {:#x} - {:#x}", stack_bottom.as_u64(), stack_top.as_u64());
        
        for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for exec stack")?;
            
            // Map into the NEW process page table with user-accessible permissions
            new_page_table.map_page(
                page, 
                frame,
                x86_64::structures::paging::PageTableFlags::PRESENT 
                    | x86_64::structures::paging::PageTableFlags::WRITABLE 
                    | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE
            )?;
        }
        
        // For now, we'll use a dummy stack object since we manually mapped the stack
        // In the future, we should refactor stack allocation to support mapping into specific page tables
        let new_stack = crate::memory::stack::allocate_stack(4096)  // Dummy size - we already mapped the real stack
            .map_err(|_| "Failed to create stack object")?;
        
        // Use our manually calculated stack top
        let new_stack_top = stack_top;
        
        log::info!("exec_process: New entry point: {:#x}, new stack top: {:#x}", 
                   new_entry_point, new_stack_top);
        
        // Update the process with new program data
        // Preserve the process ID and thread ID but replace everything else
        process.name = format!("exec_{}", pid.as_u64());
        process.entry_point = loaded_elf.entry_point;
        
        // Replace the page table with the new one containing the loaded program
        process.page_table = Some(new_page_table);
        
        // Replace the stack
        process.stack = Some(Box::new(new_stack));
        
        // Update the main thread context for the new program
        if let Some(ref mut thread) = process.main_thread {
            // CRITICAL: Preserve the kernel stack - userspace threads need it for syscalls
            let preserved_kernel_stack_top = thread.kernel_stack_top;
            log::info!("exec_process: Preserving kernel stack top: {:?}", preserved_kernel_stack_top);
            
            // Reset the CPU context for the new program
            thread.context.rip = new_entry_point;
            thread.context.rsp = new_stack_top.as_u64();
            thread.context.rflags = 0x202; // Enable interrupts
            thread.stack_top = new_stack_top;
            thread.stack_bottom = stack_bottom;
            
            // CRITICAL: Restore the preserved kernel stack - exec() doesn't change kernel stack
            thread.kernel_stack_top = preserved_kernel_stack_top;
            
            // Clear all other registers for security
            thread.context.rax = 0;
            thread.context.rbx = 0;
            thread.context.rcx = 0;
            thread.context.rdx = 0;
            thread.context.rsi = 0;
            thread.context.rdi = 0;
            thread.context.rbp = 0;
            thread.context.r8 = 0;
            thread.context.r9 = 0;
            thread.context.r10 = 0;
            thread.context.r11 = 0;
            thread.context.r12 = 0;
            thread.context.r13 = 0;
            thread.context.r14 = 0;
            thread.context.r15 = 0;
            
            // CRITICAL OS-STANDARD: Set proper segment selectors for userspace
            // These must match what the GDT defines
            thread.context.cs = 0x33; // User code segment (GDT index 6, ring 3)
            thread.context.ss = 0x2b; // User data segment (GDT index 5, ring 3)
            
            // Mark the thread as ready to run the new program
            thread.state = crate::task::thread::ThreadState::Ready;
            
            log::info!("exec_process: Updated thread {} context for new program", thread_id);
        }
        
        log::info!("exec_process: Successfully replaced process {} address space", pid.as_u64());
        
        // CRITICAL OS-STANDARD: Handle page table switching based on process state
        if is_current_process {
            // This is the current process - we're in a syscall from it
            // In a real OS, exec() on the current process requires:
            // 1. The page table switch MUST be deferred until interrupt return
            // 2. We CANNOT switch page tables while executing kernel code
            // 3. The syscall return path will handle the actual switch
            
            // Schedule the page table switch for when we return to userspace
            // This is the ONLY safe way to do it - switching while in kernel mode would crash
            unsafe {
                // This will be picked up by the interrupt return path
                crate::interrupts::context_switch::NEXT_PAGE_TABLE = 
                    process.page_table.as_ref().map(|pt| pt.level_4_frame());
            }
            
            log::info!("exec_process: Current process exec - page table switch scheduled for interrupt return");
            
            // DO NOT flush TLB here - let the interrupt return path handle it
            // Flushing TLB while still using the old page table mappings is dangerous
            // The assembly code will handle the TLB flush after the page table switch
        } else if is_scheduled {
            // Process is scheduled but not current - it will pick up the new page table
            // when it's next scheduled to run. The context switch code will handle it.
            log::info!("exec_process: Process {} is scheduled - new page table will be used on next schedule", pid.as_u64());
            // No need to set NEXT_PAGE_TABLE - the scheduler will use the process's page table
        } else {
            // Process is not scheduled - it will use the new page table when it runs
            log::info!("exec_process: Process {} is not scheduled - new page table ready for when it runs", pid.as_u64());
        }
        
        // Clean up old page table resources
        if let Some(_old_pt) = old_page_table {
            // TODO: Properly free all frames mapped by the old page table
            // This requires walking the page table and deallocating frames
            log::info!("exec_process: Old page table cleanup needed (TODO)");
        }
        
        // Add the process back to the ready queue if it's not already there
        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
            log::info!("exec_process: Added process {} back to ready queue", pid.as_u64());
        }
        
        // CRITICAL OS-STANDARD: exec() should NEVER return to the calling process
        // The process has been completely replaced. In a real implementation:
        // - If exec() succeeds, it never returns (jumps to new program)
        // - If exec() fails, it returns an error to the original program
        // For now, we return the entry point for testing, but this violates POSIX
        Ok(new_entry_point)
    }
}