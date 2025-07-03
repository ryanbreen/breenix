//! Process manager - handles process lifecycle and scheduling

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use core::sync::atomic::{self, AtomicU64, Ordering};
use x86_64::VirtAddr;

use super::{Process, ProcessId};
use crate::elf;
use crate::task::thread::Thread;
use crate::memory::stack::GuardedStack;

/// Process manager handles all processes in the system
pub struct ProcessManager {
    /// All processes indexed by PID
    processes: BTreeMap<ProcessId, Process>,
    
    /// Currently running process
    current_pid: Option<ProcessId>,
    
    /// Next available PID
    next_pid: AtomicU64,
    
    /// Queue of ready processes
    ready_queue: Vec<ProcessId>,
    
    /// Next available process base address (for virtual address allocation)
    next_process_base: VirtAddr,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        ProcessManager {
            processes: BTreeMap::new(),
            current_pid: None,
            next_pid: AtomicU64::new(1), // PIDs start at 1 (0 is kernel)
            ready_queue: Vec::new(),
            // Start process virtual addresses at 0x10000000, with 16MB spacing
            next_process_base: VirtAddr::new(0x10000000),
        }
    }
    
    /// Create a new process from an ELF binary
    pub fn create_process(&mut self, name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
        // Allocate a virtual address range for this process
        let process_base = self.next_process_base;
        self.next_process_base += 0x1000000; // 16MB per process
        
        // Load the ELF binary at the allocated base address
        let loaded_elf = elf::load_elf_at_base(elf_data, process_base)?;
        
        // Generate a new PID
        let pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        
        // Create the process
        let mut process = Process::new(pid, name.clone(), loaded_elf.entry_point);
        
        // Update memory usage
        process.memory_usage.code_size = elf_data.len();
        
        // Allocate a stack for the process
        use crate::memory::stack;
        use crate::task::thread::ThreadPrivilege;
        
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        let user_stack = stack::allocate_stack_with_privilege(
            USER_STACK_SIZE,
            ThreadPrivilege::User
        ).map_err(|_| "Failed to allocate user stack")?;
        
        let stack_top = user_stack.top();
        process.memory_usage.stack_size = USER_STACK_SIZE;
        
        // Store the stack in the process
        process.stack = Some(Box::new(user_stack));
        
        // Create the main thread
        let thread = self.create_main_thread(&process, stack_top)?;
        process.set_main_thread(thread);
        
        // Add to ready queue
        self.ready_queue.push(pid);
        
        // Insert into process table
        self.processes.insert(pid, process);
        
        log::info!("Created process {} (PID {})", name, pid.as_u64());
        
        Ok(pid)
    }
    
    /// Create the main thread for a process
    fn create_main_thread(&self, process: &Process, stack_top: x86_64::VirtAddr) -> Result<Thread, &'static str> {
        
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
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: crate::task::thread::ThreadPrivilege::User,
        };
        
        Ok(thread)
    }
    
    /// Get the current process ID
    pub fn current_pid(&self) -> Option<ProcessId> {
        self.current_pid
    }
    
    /// Set the current process ID (for direct execution)
    pub fn set_current_pid(&mut self, pid: ProcessId) {
        self.current_pid = Some(pid);
        
        // Update process state
        if let Some(process) = self.processes.get_mut(&pid) {
            process.set_running();
        }
    }
    
    /// Get a reference to a process
    pub fn get_process(&self, pid: ProcessId) -> Option<&Process> {
        self.processes.get(&pid)
    }
    
    /// Get a mutable reference to a process
    pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
        self.processes.get_mut(&pid)
    }
    
    /// Exit a process with the given exit code
    pub fn exit_process(&mut self, pid: ProcessId, exit_code: i32) {
        if let Some(process) = self.processes.get_mut(&pid) {
            log::info!("Process {} (PID {}) exiting with code {}", 
                process.name, pid.as_u64(), exit_code);
            
            process.terminate(exit_code);
            
            // Remove from ready queue
            self.ready_queue.retain(|&p| p != pid);
            
            // If this was the current process, clear it
            if self.current_pid == Some(pid) {
                self.current_pid = None;
            }
            
            // TODO: Clean up process resources
            // - Unmap memory pages
            // - Close file descriptors
            // - Signal waiting processes
            // - Reparent children to init
        }
    }
    
    /// Get the next ready process to run
    pub fn schedule_next(&mut self) -> Option<ProcessId> {
        // Simple round-robin for now
        if let Some(pid) = self.ready_queue.first().cloned() {
            // Move to back of queue
            self.ready_queue.remove(0);
            self.ready_queue.push(pid);
            
            // Update states
            if let Some(old_pid) = self.current_pid {
                if let Some(old_process) = self.processes.get_mut(&old_pid) {
                    if !old_process.is_terminated() {
                        old_process.set_ready();
                    }
                }
            }
            
            if let Some(new_process) = self.processes.get_mut(&pid) {
                new_process.set_running();
            }
            
            self.current_pid = Some(pid);
            Some(pid)
        } else {
            None
        }
    }
    
    /// Get all process IDs
    pub fn all_pids(&self) -> Vec<ProcessId> {
        self.processes.keys().cloned().collect()
    }
    
    /// Get process count
    pub fn process_count(&self) -> usize {
        self.processes.len()
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
        log::info!("Current PID: {:?}", self.current_pid);
        log::info!("Ready queue: {:?}", self.ready_queue);
    }
    
    /// Fork a process - create a child process that's a copy of the parent
    pub fn fork_process(&mut self, parent_pid: ProcessId) -> Result<ProcessId, &'static str> {
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
        
        // TODO: For now, we'll create a simple copy. In the future, we need:
        // 1. Copy-on-write memory pages
        // 2. Duplicate file descriptors
        // 3. Copy signal handlers
        // 4. Copy other process state
        
        // Create a new stack for the child thread
        // Create a new stack for the child process (64KB userspace stack)
        let mut mapper = unsafe { crate::memory::paging::get_mapper() };
        let child_stack = GuardedStack::new(64 * 1024, &mut mapper, crate::task::thread::ThreadPrivilege::User)
            .map_err(|_| "Failed to allocate stack for child process")?;
        let child_stack_top = child_stack.top();
        
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
        
        // Create the child thread manually to use specific ID
        let mut child_thread = Thread {
            id: child_thread_id,
            name: child_name,
            state: crate::task::thread::ThreadState::Ready,
            context: parent_thread.context.clone(), // Will be modified below
            stack_top: child_stack_top,
            stack_bottom: child_stack_top - (64 * 1024),
            tls_block: child_tls_block,
            priority: parent_thread.priority,
            time_slice: parent_thread.time_slice,
            entry_point: None, // Userspace threads don't have kernel entry points
            privilege: parent_thread.privilege,
        };
        // But update the stack pointer to use the child's stack
        child_thread.context.rsp = child_stack_top.as_u64();
        // IMPORTANT: Set RAX to 0 for the child (fork return value)
        child_thread.context.rax = 0;
        
        // Set up child thread properties
        child_thread.privilege = parent_thread.privilege;
        // Mark child as ready to run
        child_thread.state = crate::task::thread::ThreadState::Ready;
        
        // Store the stack in the child process
        child_process.stack = Some(Box::new(child_stack));
        
        // Set the child thread as the main thread of the child process
        child_process.set_main_thread(child_thread);
        
        // Add child to parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.add_child(child_pid);
        }
        
        // Add the child process to the process table
        self.processes.insert(child_pid, child_process);
        
        // Add the child to the ready queue so it can be scheduled
        self.ready_queue.push(child_pid);
        
        log::info!("Fork complete: parent {} -> child {}", 
            parent_pid.as_u64(), child_pid.as_u64());
        
        // Return the child PID to the parent
        Ok(child_pid)
    }
}