//! Process structure and lifecycle

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use x86_64::VirtAddr;
use crate::task::thread::Thread;
use crate::memory::stack::GuardedStack;
use crate::memory::process_memory::ProcessPageTable;

/// Process ID type
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessId(u64);

impl ProcessId {
    pub fn new(id: u64) -> Self {
        ProcessId(id)
    }
    
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Process state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is being created
    Creating,
    /// Process is ready to run
    Ready,
    /// Process is currently running
    Running,
    /// Process is blocked waiting for something
    Blocked,
    /// Process has terminated
    Terminated(i32), // exit code
}

/// A process represents a running program with its own address space
pub struct Process {
    /// Unique process identifier
    pub id: ProcessId,
    
    /// Process name (for debugging)
    pub name: String,
    
    /// Current state
    pub state: ProcessState,
    
    /// Entry point address
    pub entry_point: VirtAddr,
    
    /// Main thread of the process
    pub main_thread: Option<Thread>,
    
    /// Additional threads (for future multi-threading support)
    #[allow(dead_code)]
    pub threads: Vec<u64>, // Thread IDs
    
    /// Parent process ID (if any)
    pub parent: Option<ProcessId>,
    
    /// Child processes
    pub children: Vec<ProcessId>,
    
    /// Exit code (if terminated)
    pub exit_code: Option<i32>,
    
    /// Memory usage statistics
    pub memory_usage: MemoryUsage,
    
    /// Stack allocated for this process
    pub stack: Option<Box<GuardedStack>>,
    
    /// Per-process page table
    pub page_table: Option<Box<ProcessPageTable>>,
}

/// Memory usage tracking
#[derive(Debug, Default)]
pub struct MemoryUsage {
    /// Size of loaded program segments in bytes
    pub code_size: usize,
    /// Size of allocated heap in bytes
    #[allow(dead_code)]
    pub heap_size: usize,
    /// Size of allocated stack in bytes
    pub stack_size: usize,
}

impl Process {
    /// Create a new process
    pub fn new(id: ProcessId, name: String, entry_point: VirtAddr) -> Self {
        Process {
            id,
            name,
            state: ProcessState::Creating,
            entry_point,
            main_thread: None,
            threads: Vec::new(),
            parent: None,
            children: Vec::new(),
            exit_code: None,
            memory_usage: MemoryUsage::default(),
            stack: None,
            page_table: None,
        }
    }
    
    /// Set the main thread for this process
    pub fn set_main_thread(&mut self, thread: Thread) {
        self.main_thread = Some(thread);
        self.state = ProcessState::Ready;
    }
    
    /// Mark process as running
    pub fn set_running(&mut self) {
        self.state = ProcessState::Running;
    }
    
    /// Mark process as blocked
    #[allow(dead_code)]
    pub fn set_blocked(&mut self) {
        self.state = ProcessState::Blocked;
    }
    
    /// Mark process as ready
    pub fn set_ready(&mut self) {
        self.state = ProcessState::Ready;
    }
    
    /// Terminate the process
    pub fn terminate(&mut self, exit_code: i32) {
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
    }
    
    /// Check if process is terminated
    pub fn is_terminated(&self) -> bool {
        matches!(self.state, ProcessState::Terminated(_))
    }
    
    /// Add a child process
    pub fn add_child(&mut self, child_id: ProcessId) {
        self.children.push(child_id);
    }
    
    /// Remove a child process
    #[allow(dead_code)]
    pub fn remove_child(&mut self, child_id: ProcessId) {
        self.children.retain(|&id| id != child_id);
    }
}