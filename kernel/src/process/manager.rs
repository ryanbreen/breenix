//! Process manager - handles process lifecycle and scheduling

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;

use super::{Process, ProcessId};
use crate::elf;
use crate::task::thread::Thread;

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
        let tls_block = x86_64::VirtAddr::new(0);
        
        // Create a thread for the process
        let thread = Thread::new_userspace(
            String::from(&process.name),
            process.entry_point,
            stack_top,
            tls_block,
        );
        
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
}