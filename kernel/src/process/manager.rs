//! Process manager - handles process lifecycle and scheduling

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// Import paging types from appropriate source
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;

use super::{Process, ProcessId};
#[cfg(target_arch = "x86_64")]
use crate::elf;
use crate::memory::process_memory::ProcessPageTable;
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
    #[allow(dead_code)]
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
            // Start process virtual addresses at USERSPACE_BASE, with 16MB spacing
            next_process_base: VirtAddr::new(crate::memory::layout::USERSPACE_BASE),
        }
    }

    /// Create a new process from an ELF binary
    /// Note: Uses x86_64-specific ELF loader
    #[cfg(target_arch = "x86_64")]
    pub fn create_process(
        &mut self,
        name: String,
        elf_data: &[u8],
    ) -> Result<ProcessId, &'static str> {
        crate::serial_println!("manager.create_process: ENTRY - name='{}', elf_size={}", name, elf_data.len());

        // Generate a new PID
        crate::serial_println!("manager.create_process: Generating PID");
        let pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        crate::serial_println!("manager.create_process: Generated PID {}", pid.as_u64());

        // Create a new page table for this process
        crate::serial_println!("manager.create_process: Creating ProcessPageTable");
        let mut page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new().map_err(|e| {
                log::error!(
                    "Failed to create process page table for PID {}: {}",
                    pid.as_u64(),
                    e
                );
                crate::serial_println!("manager.create_process: ProcessPageTable creation failed: {}", e);
                "Failed to create process page table"
            })?,
        );
        crate::serial_println!("manager.create_process: ProcessPageTable created");

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
        crate::serial_println!("manager.create_process: Loading ELF into page table");
        let loaded_elf = elf::load_elf_into_page_table(elf_data, page_table.as_mut())?;
        crate::serial_println!("manager.create_process: ELF loaded, entry={:#x}", loaded_elf.entry_point.as_u64());

        // CRITICAL FIX: Re-map kernel low-half after ELF loading
        // The ELF loader may have created new page tables that don't preserve kernel mappings
        // We need to explicitly ensure the kernel code/data remains mapped
        // Note: This is x86_64-specific due to kernel memory layout differences
        #[cfg(target_arch = "x86_64")]
        {
            use x86_64::VirtAddr;
            use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

            log::info!("Restoring kernel mappings after ELF load...");
            crate::serial_println!("manager.create_process: Restoring kernel mappings");

            // CRITICAL: The kernel is running from the direct physical memory mapping,
            // NOT from the low identity-mapped region!
            // We need to preserve the direct mapping where the kernel actually executes.
            //
            // Based on RIP=0x10000068f65, the kernel is in the 0x100000xxxxx range
            // which is the direct physical memory mapping starting at PHYS_MEM_OFFSET
            //
            // Actually, let's just ensure the kernel's actual physical addresses are mapped
            // Map kernel code/data region: 0x100000 - 0x300000 (2MB physical)
            let kernel_start = 0x100000u64;
            let kernel_end = 0x300000u64;

            for addr in (kernel_start..kernel_end).step_by(0x1000) {
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
                let frame = PhysFrame::<Size4KiB>::containing_address(x86_64::PhysAddr::new(addr));

                // Check if already mapped correctly
                if let Some(existing_frame) = page_table.translate_page(VirtAddr::new(addr)) {
                    if existing_frame.as_u64() == addr {
                        continue; // Already mapped correctly
                    }
                }

                // Map with kernel-only access (no USER_ACCESSIBLE)
                let flags = if addr < 0x200000 {
                    // Text section - read-only, executable
                    PageTableFlags::PRESENT | PageTableFlags::GLOBAL
                } else {
                    // Data/BSS sections - read-write
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL
                };

                if let Err(e) = page_table.map_page(page, frame, flags) {
                    log::error!("Failed to restore kernel mapping at {:#x}: {}", addr, e);
                    return Err("Failed to restore kernel mappings");
                }
            }

            // Also map GDT/IDT/TSS/per-CPU region: 0x100000f0000 - 0x100000f4000
            let control_start = 0x100000f0000u64;
            let control_end = 0x100000f4000u64;

            for addr in (control_start..control_end).step_by(0x1000) {
                let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
                let frame = PhysFrame::<Size4KiB>::containing_address(x86_64::PhysAddr::new(addr));

                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;

                // Ignore if already mapped
                if page_table.translate_page(VirtAddr::new(addr)).is_some() {
                    continue;
                }

                if let Err(e) = page_table.map_page(page, frame, flags) {
                    log::error!("Failed to map control structure at {:#x}: {}", addr, e);
                    // Non-fatal, continue
                }
            }

            log::info!("✓ Kernel low-half mappings restored");
            crate::serial_println!("manager.create_process: Kernel mappings restored");

            // Verify the mapping actually worked
            let kernel_test_addr = VirtAddr::new(0x100000);
            match page_table.translate_page(kernel_test_addr) {
                Some(phys_addr) => {
                    log::info!("✓✓ VERIFIED: Kernel at {:#x} -> {:#x} after restoration",
                             kernel_test_addr.as_u64(), phys_addr.as_u64());
                },
                None => {
                    log::error!("✗✗ CRITICAL: Kernel still not mapped after restoration!");
                    return Err("Kernel restoration failed!");
                }
            }
        }

        // Create the process
        crate::serial_println!("manager.create_process: Creating Process struct");
        let mut process = Process::new(pid, name.clone(), loaded_elf.entry_point);
        process.page_table = Some(page_table);

        // Initialize heap tracking - heap starts at end of loaded segments (page aligned)
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;
        crate::serial_println!("manager.create_process: Process struct created, heap_start={:#x}", heap_base);

        // Update memory usage
        process.memory_usage.code_size = elf_data.len();

        // Allocate a stack for the process
        use crate::memory::stack;
        use crate::task::thread::ThreadPrivilege;

        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        crate::serial_println!("manager.create_process: Allocating user stack");
        let user_stack =
            stack::allocate_stack_with_privilege(USER_STACK_SIZE, ThreadPrivilege::User)
                .map_err(|_| {
                    crate::serial_println!("manager.create_process: Stack allocation failed");
                    "Failed to allocate user stack"
                })?;
        crate::serial_println!("manager.create_process: User stack allocated at {:#x}", user_stack.top());

        let stack_top = user_stack.top();
        process.memory_usage.stack_size = USER_STACK_SIZE;

        // Store the stack in the process
        process.stack = Some(Box::new(user_stack));

        // CRITICAL: Map the user stack pages into the process page table
        // The stack was allocated in the kernel page table, but userspace needs it mapped
        log::debug!("Mapping user stack pages into process page table...");
        crate::serial_println!("manager.create_process: Mapping user stack into process page table");
        if let Some(ref mut page_table) = process.page_table {
            let stack_bottom = stack_top - USER_STACK_SIZE as u64;
            crate::memory::process_memory::map_user_stack_to_process(
                page_table,
                stack_bottom,
                stack_top,
            )
            .map_err(|e| {
                log::error!("Failed to map user stack to process page table: {}", e);
                "Failed to map user stack in process page table"
            })?;
            log::debug!("✓ User stack mapped in process page table");
            crate::serial_println!("manager.create_process: User stack mapped successfully");
        } else {
            return Err("Process page table not available for stack mapping");
        }

        // Create the main thread
        crate::serial_println!("manager.create_process: Creating main thread");
        let thread = self.create_main_thread(&mut process, stack_top)?;
        crate::serial_println!("manager.create_process: Main thread created");
        process.set_main_thread(thread);
        crate::serial_println!("manager.create_process: Main thread set on process");

        // Add to ready queue
        crate::serial_println!("manager.create_process: Adding PID {} to ready queue", pid.as_u64());
        self.ready_queue.push(pid);

        // Insert into process table
        crate::serial_println!("manager.create_process: Inserting process into process table");
        self.processes.insert(pid, process);

        log::info!("Created process {} (PID {})", name, pid.as_u64());
        crate::serial_println!("manager.create_process: SUCCESS - returning PID {}", pid.as_u64());

        Ok(pid)
    }

    /// Create a new process from an ELF binary (ARM64 version)
    ///
    /// This is simpler than the x86_64 version because:
    /// - ARM64 uses TTBR1 for kernel mappings automatically (no kernel mapping restoration needed)
    /// - Uses ARM64-specific ELF loader
    /// - Uses ARM64-specific ProcessPageTable
    #[cfg(target_arch = "aarch64")]
    pub fn create_process(
        &mut self,
        name: String,
        elf_data: &[u8],
    ) -> Result<ProcessId, &'static str> {
        // For ARM64, stack allocation uses arch_stub::ThreadPrivilege
        use crate::memory::arch_stub::ThreadPrivilege as StackPrivilege;

        crate::serial_println!("manager.create_process [ARM64]: ENTRY - name='{}', elf_size={}", name, elf_data.len());

        // Generate a new PID
        let pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        crate::serial_println!("manager.create_process [ARM64]: Generated PID {}", pid.as_u64());

        // Create a new page table for this process
        // On ARM64, this creates a TTBR0 page table for userspace only
        // Kernel mappings are handled automatically via TTBR1
        crate::serial_println!("manager.create_process [ARM64]: Creating ProcessPageTable");
        let mut page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new().map_err(|e| {
                log::error!(
                    "ARM64: Failed to create process page table for PID {}: {}",
                    pid.as_u64(),
                    e
                );
                crate::serial_println!("manager.create_process [ARM64]: ProcessPageTable creation failed: {}", e);
                "Failed to create process page table"
            })?,
        );
        crate::serial_println!("manager.create_process [ARM64]: ProcessPageTable created");

        // Load the ELF binary into the process's page table
        // Use the ARM64-specific ELF loader
        crate::serial_println!("manager.create_process [ARM64]: Loading ELF into page table");
        let loaded_elf = crate::arch_impl::aarch64::elf::load_elf_into_page_table(
            elf_data,
            page_table.as_mut(),
        )?;
        crate::serial_println!(
            "manager.create_process [ARM64]: ELF loaded, entry={:#x}",
            loaded_elf.entry_point
        );

        // NOTE: On ARM64, we skip kernel mapping restoration because:
        // - TTBR1_EL1 always holds kernel mappings (upper half addresses: 0xFFFF...)
        // - TTBR0_EL1 holds userspace mappings (lower half addresses: 0x0000...)
        // - The hardware automatically selects the correct translation table based on address
        // This is a key simplification compared to x86_64 where CR3 holds all mappings

        // Create the process
        crate::serial_println!("manager.create_process [ARM64]: Creating Process struct");
        let entry_point = VirtAddr::new(loaded_elf.entry_point);
        let mut process = Process::new(pid, name.clone(), entry_point);
        process.page_table = Some(page_table);

        // Initialize heap tracking - heap starts at end of loaded segments (page aligned)
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;
        crate::serial_println!(
            "manager.create_process [ARM64]: Process struct created, heap_start={:#x}",
            heap_base
        );

        // Update memory usage
        process.memory_usage.code_size = elf_data.len();

        // Allocate physical frames for the stack
        // On ARM64, we need to:
        // 1. Allocate physical memory for the stack
        // 2. Map it at USERSPACE addresses (not kernel HHDM addresses)
        // 3. Use the userspace addresses for the thread's SP
        use crate::memory::stack;
        use crate::arch_impl::aarch64::constants::USER_STACK_REGION_START;

        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        crate::serial_println!("manager.create_process [ARM64]: Allocating user stack");

        // allocate_stack_with_privilege returns HHDM addresses (kernel-accessible)
        // We need to extract the physical frames and map them to userspace addresses
        let kernel_stack =
            stack::allocate_stack_with_privilege(USER_STACK_SIZE, StackPrivilege::User)
                .map_err(|_| {
                    crate::serial_println!("manager.create_process [ARM64]: Stack allocation failed");
                    "Failed to allocate user stack"
                })?;

        // The kernel_stack.top() is an HHDM address - extract physical address
        let kernel_stack_top = kernel_stack.top().as_u64();
        let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
        let stack_phys_top = kernel_stack_top - hhdm_base;
        let stack_phys_bottom = stack_phys_top - USER_STACK_SIZE as u64;

        crate::serial_println!(
            "manager.create_process [ARM64]: Stack physical range {:#x}-{:#x}",
            stack_phys_bottom,
            stack_phys_top
        );

        // Calculate userspace stack addresses
        // Stack grows down, so stack_top is the highest address
        let user_stack_top = USER_STACK_REGION_START;
        let user_stack_bottom = user_stack_top - USER_STACK_SIZE as u64;

        crate::serial_println!(
            "manager.create_process [ARM64]: User stack will be at {:#x}-{:#x}",
            user_stack_bottom,
            user_stack_top
        );

        process.memory_usage.stack_size = USER_STACK_SIZE;

        // Store the kernel-accessible stack (for potential kernel access later)
        process.stack = Some(Box::new(kernel_stack));

        // Map the physical stack frames into the process page table at USERSPACE addresses
        log::debug!("ARM64: Mapping user stack pages into process page table...");
        crate::serial_println!("manager.create_process [ARM64]: Mapping user stack into process page table");
        if let Some(ref mut page_table) = process.page_table {
            crate::serial_println!(
                "manager.create_process [ARM64]: map_user_stack_to_process user_bottom={:#x} user_top={:#x} phys_bottom={:#x}",
                user_stack_bottom,
                user_stack_top,
                stack_phys_bottom
            );

            // Map physical frames to userspace addresses
            crate::memory::process_memory::map_user_stack_to_process_with_phys(
                page_table,
                VirtAddr::new(user_stack_bottom),
                VirtAddr::new(user_stack_top),
                stack_phys_bottom,
            )
            .map_err(|e| {
                crate::serial_println!(
                    "manager.create_process [ARM64]: map_user_stack_to_process FAILED: {}",
                    e
                );
                log::error!("ARM64: Failed to map user stack to process page table: {}", e);
                "Failed to map user stack in process page table"
            })?;
            log::debug!("ARM64: User stack mapped in process page table");
            crate::serial_println!("manager.create_process [ARM64]: User stack mapped successfully");
        } else {
            return Err("Process page table not available for stack mapping");
        }

        // Create the main thread with USERSPACE stack top
        crate::serial_println!("manager.create_process [ARM64]: Creating main thread");
        let user_stack_top_vaddr = VirtAddr::new(user_stack_top);
        let thread = self.create_main_thread(&mut process, user_stack_top_vaddr)?;
        crate::serial_println!("manager.create_process [ARM64]: Main thread created");
        process.set_main_thread(thread);
        crate::serial_println!("manager.create_process [ARM64]: Main thread set on process");

        // Add to ready queue
        crate::serial_println!(
            "manager.create_process [ARM64]: Adding PID {} to ready queue",
            pid.as_u64()
        );
        self.ready_queue.push(pid);

        // Insert into process table
        crate::serial_println!("manager.create_process [ARM64]: Inserting process into process table");
        self.processes.insert(pid, process);

        log::info!("ARM64: Created process {} (PID {})", name, pid.as_u64());
        crate::serial_println!(
            "manager.create_process [ARM64]: SUCCESS - returning PID {}",
            pid.as_u64()
        );

        Ok(pid)
    }

    /// Create a new process from an ELF binary with argc/argv support (ARM64 version)
    ///
    /// This is like `create_process` but also sets up argc/argv on the stack
    /// following the Linux ABI convention:
    ///
    /// Stack layout at _start (from high to low addresses):
    /// - argv strings (null-terminated)
    /// - padding for 16-byte alignment
    /// - NULL (end of argv pointers)
    /// - argv[n-1] pointer
    /// - ...
    /// - argv[0] pointer
    /// - argc           <- SP points here
    ///
    /// Parameters:
    /// - name: Process name
    /// - elf_data: The ELF binary data
    /// - argv: Array of argument strings (argv[0] is typically the program name)
    ///
    /// Returns: ProcessId on success
    #[cfg(target_arch = "aarch64")]
    pub fn create_process_with_argv(
        &mut self,
        name: String,
        elf_data: &[u8],
        argv: &[&[u8]],
    ) -> Result<ProcessId, &'static str> {
        // For ARM64, stack allocation uses arch_stub::ThreadPrivilege
        use crate::memory::arch_stub::ThreadPrivilege as StackPrivilege;

        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: ENTRY - name='{}', elf_size={}, argc={}",
            name,
            elf_data.len(),
            argv.len()
        );

        // Generate a new PID
        let pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: Generated PID {}",
            pid.as_u64()
        );

        // Create a new page table for this process
        crate::serial_println!("manager.create_process_with_argv [ARM64]: Creating ProcessPageTable");
        let mut page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new().map_err(|e| {
                log::error!(
                    "ARM64: Failed to create process page table for PID {}: {}",
                    pid.as_u64(),
                    e
                );
                "Failed to create process page table"
            })?,
        );

        // Load the ELF binary into the process's page table
        crate::serial_println!("manager.create_process_with_argv [ARM64]: Loading ELF into page table");
        let loaded_elf = crate::arch_impl::aarch64::elf::load_elf_into_page_table(
            elf_data,
            page_table.as_mut(),
        )?;
        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: ELF loaded, entry={:#x}",
            loaded_elf.entry_point
        );

        // Create the process
        let entry_point = VirtAddr::new(loaded_elf.entry_point);
        let mut process = Process::new(pid, name.clone(), entry_point);
        process.page_table = Some(page_table);

        // Initialize heap tracking
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;

        // Update memory usage
        process.memory_usage.code_size = elf_data.len();

        // Allocate physical frames for the stack
        use crate::memory::stack;
        use crate::arch_impl::aarch64::constants::USER_STACK_REGION_START;

        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        crate::serial_println!("manager.create_process_with_argv [ARM64]: Allocating user stack");

        let kernel_stack =
            stack::allocate_stack_with_privilege(USER_STACK_SIZE, StackPrivilege::User)
                .map_err(|_| "Failed to allocate user stack")?;

        // Extract physical address from HHDM address
        let kernel_stack_top = kernel_stack.top().as_u64();
        let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
        let stack_phys_top = kernel_stack_top - hhdm_base;
        let stack_phys_bottom = stack_phys_top - USER_STACK_SIZE as u64;

        // Calculate userspace stack addresses
        let user_stack_top = USER_STACK_REGION_START;
        let user_stack_bottom = user_stack_top - USER_STACK_SIZE as u64;

        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: User stack will be at {:#x}-{:#x}",
            user_stack_bottom,
            user_stack_top
        );

        process.memory_usage.stack_size = USER_STACK_SIZE;
        process.stack = Some(Box::new(kernel_stack));

        // Map the physical stack frames into the process page table
        if let Some(ref mut page_table) = process.page_table {
            crate::memory::process_memory::map_user_stack_to_process_with_phys(
                page_table,
                VirtAddr::new(user_stack_bottom),
                VirtAddr::new(user_stack_top),
                stack_phys_bottom,
            )
            .map_err(|e| {
                log::error!("ARM64: Failed to map user stack to process page table: {}", e);
                "Failed to map user stack in process page table"
            })?;
        } else {
            return Err("Process page table not available for stack mapping");
        }

        // Set up argc/argv on the stack following Linux ABI
        // The stack is now mapped, so we can write to it via physical addresses
        let initial_sp = if let Some(ref page_table) = process.page_table {
            self.setup_argv_on_stack(page_table, user_stack_top, argv)?
        } else {
            return Err("Process page table not available for argv setup");
        };

        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: argc/argv set up on stack, SP={:#x}",
            initial_sp
        );

        // Create the main thread with the adjusted stack pointer (pointing to argc)
        let thread = self.create_main_thread_with_sp(&mut process, VirtAddr::new(user_stack_top), VirtAddr::new(initial_sp))?;
        process.set_main_thread(thread);

        // Add to ready queue and insert into process table
        self.ready_queue.push(pid);
        self.processes.insert(pid, process);

        log::info!(
            "ARM64: Created process {} (PID {}) with argc={}",
            name,
            pid.as_u64(),
            argv.len()
        );
        crate::serial_println!(
            "manager.create_process_with_argv [ARM64]: SUCCESS - returning PID {}",
            pid.as_u64()
        );

        Ok(pid)
    }

    /// Create the main thread for a process
    /// Note: Uses x86_64-specific TLS and thread creation
    #[cfg(target_arch = "x86_64")]
    fn create_main_thread(
        &mut self,
        process: &mut Process,
        stack_top: VirtAddr,
    ) -> Result<Thread, &'static str> {
        // For now, use a null TLS block (we'll implement TLS later)
        let _tls_block = VirtAddr::new(0);

        // Allocate a globally unique thread ID
        // NOTE: While Unix convention is TID = PID for main thread, we need global
        // uniqueness across all threads (kernel + user). Using the global allocator
        // prevents collisions with kernel threads.
        let thread_id = crate::task::thread::allocate_thread_id();

        // Allocate a TLS block for this thread ID
        let actual_tls_block = VirtAddr::new(0x10000 + thread_id * 0x1000);

        // Register this thread with the TLS system (x86_64 only for now)
        #[cfg(target_arch = "x86_64")]
        if let Err(e) = crate::tls::register_thread_tls(thread_id, actual_tls_block) {
            log::warn!(
                "Failed to register thread {} with TLS system: {}",
                thread_id,
                e
            );
        }

        // Calculate stack bottom (stack grows down)
        const USER_STACK_SIZE: usize = 64 * 1024;
        let stack_bottom = stack_top - USER_STACK_SIZE as u64;

        // Allocate a kernel stack using the new global kernel stack allocator
        // This automatically maps the stack in the global kernel page tables,
        // making it visible to all processes
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack().map_err(|e| {
            log::error!("Failed to allocate kernel stack: {}", e);
            "Failed to allocate kernel stack for thread"
        })?;
        let kernel_stack_top = kernel_stack.top();

        log::debug!(
            "✓ Allocated kernel stack at {:#x} (globally visible)",
            kernel_stack_top
        );

        // Store the kernel stack - it will be dropped when the thread is destroyed
        // For now, we'll leak it - TODO: proper cleanup
        Box::leak(Box::new(kernel_stack));

        // Set up initial context for userspace
        // CRITICAL: RSP must point WITHIN the mapped stack region, not past it
        // The stack grows down, so we start RSP at (stack_top - 16) for alignment
        let initial_rsp = VirtAddr::new(stack_top.as_u64() - 16);
        let context = crate::task::thread::CpuContext::new(
            process.entry_point,
            initial_rsp,
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
            kernel_stack_allocation: None, // Kernel stack for userspace thread not managed here
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: crate::task::thread::ThreadPrivilege::User,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        };

        Ok(thread)
    }

    /// Create the main thread for a process (ARM64 version)
    ///
    /// Note: TLS support is not yet implemented for ARM64.
    #[cfg(target_arch = "aarch64")]
    fn create_main_thread(
        &mut self,
        process: &mut Process,
        stack_top: VirtAddr,
    ) -> Result<Thread, &'static str> {
        // Allocate a globally unique thread ID
        let thread_id = crate::task::thread::allocate_thread_id();

        // For ARM64, use a simple TLS placeholder (TLS not yet fully implemented)
        let actual_tls_block = VirtAddr::new(0x10000 + thread_id * 0x1000);

        // Calculate stack bottom (stack grows down)
        const USER_STACK_SIZE: usize = 64 * 1024;
        let stack_bottom = VirtAddr::new(stack_top.as_u64() - USER_STACK_SIZE as u64);

        // Allocate a kernel stack for exception handling
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack().map_err(|e| {
            log::error!("ARM64: Failed to allocate kernel stack: {}", e);
            "Failed to allocate kernel stack for thread"
        })?;
        let kernel_stack_top = kernel_stack.top();

        log::debug!(
            "ARM64: Allocated kernel stack at {:#x}",
            kernel_stack_top.as_u64()
        );

        // Store the kernel stack - it will be dropped when the thread is destroyed
        // For now, we'll leak it - TODO: proper cleanup
        Box::leak(Box::new(kernel_stack));

        // Set up initial context for userspace
        // On ARM64, SP should be 16-byte aligned
        let initial_sp = VirtAddr::new(stack_top.as_u64() & !0xF);
        let context = crate::task::thread::CpuContext::new(
            process.entry_point,
            initial_sp,
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
            kernel_stack_allocation: None, // Kernel stack for userspace thread not managed here
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: crate::task::thread::ThreadPrivilege::User,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        };

        Ok(thread)
    }

    /// Create the main thread for a process with a specific initial SP (ARM64 version)
    ///
    /// This is used when argc/argv has been set up on the stack, so the initial SP
    /// needs to point to argc rather than the top of the stack.
    ///
    /// Parameters:
    /// - process: The process to create the thread for
    /// - stack_top: The top of the stack region (highest address)
    /// - initial_sp: The initial SP value (pointing to argc on the stack)
    #[cfg(target_arch = "aarch64")]
    fn create_main_thread_with_sp(
        &mut self,
        process: &mut Process,
        stack_top: VirtAddr,
        initial_sp: VirtAddr,
    ) -> Result<Thread, &'static str> {
        // Allocate a globally unique thread ID
        let thread_id = crate::task::thread::allocate_thread_id();

        // For ARM64, use a simple TLS placeholder (TLS not yet fully implemented)
        let actual_tls_block = VirtAddr::new(0x10000 + thread_id * 0x1000);

        // Calculate stack bottom (stack grows down)
        const USER_STACK_SIZE: usize = 64 * 1024;
        let stack_bottom = VirtAddr::new(stack_top.as_u64() - USER_STACK_SIZE as u64);

        // Allocate a kernel stack for exception handling
        let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack().map_err(|e| {
            log::error!("ARM64: Failed to allocate kernel stack: {}", e);
            "Failed to allocate kernel stack for thread"
        })?;
        let kernel_stack_top = kernel_stack.top();

        log::debug!(
            "ARM64: Allocated kernel stack at {:#x}",
            kernel_stack_top.as_u64()
        );

        // Store the kernel stack - it will be dropped when the thread is destroyed
        Box::leak(Box::new(kernel_stack));

        // Set up initial context for userspace
        // Use the provided initial_sp which points to argc on the stack
        // setup_argv_on_stack() already ensures 16-byte alignment (ARM64 ABI requirement)
        debug_assert!(
            initial_sp.as_u64() & 0xF == 0,
            "initial_sp must be 16-byte aligned"
        );
        let context = crate::task::thread::CpuContext::new(
            process.entry_point,
            initial_sp,
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
            kernel_stack_allocation: None,
            tls_block: actual_tls_block,
            priority: 128,
            time_slice: 10,
            entry_point: None,
            privilege: crate::task::thread::ThreadPrivilege::User,
            has_started: false,
            blocked_in_syscall: false,
            saved_userspace_context: None,
            wake_time_ns: None,
        };

        Ok(thread)
    }

    /// Get the current process ID
    #[allow(dead_code)]
    pub fn current_pid(&self) -> Option<ProcessId> {
        self.current_pid
    }

    /// Set the current process ID (for direct execution)
    #[allow(dead_code)]
    pub fn set_current_pid(&mut self, pid: ProcessId) {
        self.current_pid = Some(pid);

        // Update process state
        if let Some(process) = self.processes.get_mut(&pid) {
            process.set_running();
        }
    }

    /// Allocate a new process ID
    pub fn allocate_pid(&self) -> ProcessId {
        ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst))
    }

    /// Insert a fully-constructed process into the manager
    pub fn insert_process(&mut self, pid: ProcessId, process: Process) {
        self.processes.insert(pid, process);
    }

    /// Get a reference to a process
    #[allow(dead_code)]
    pub fn get_process(&self, pid: ProcessId) -> Option<&Process> {
        self.processes.get(&pid)
    }

    /// Get a mutable reference to a process
    #[allow(dead_code)]
    pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
        self.processes.get_mut(&pid)
    }

    /// Exit a process with the given exit code
    #[allow(dead_code)]
    pub fn exit_process(&mut self, pid: ProcessId, exit_code: i32) {
        // Get parent PID before we borrow the process mutably
        let parent_pid = self.processes.get(&pid).and_then(|p| p.parent);

        if let Some(process) = self.processes.get_mut(&pid) {
            log::info!(
                "Process {} (PID {}) exiting with code {}",
                process.name,
                pid.as_u64(),
                exit_code
            );

            // Drain any pending old page tables from previous exec() calls
            process.drain_old_page_tables();

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
            // - Reparent children to init
        }

        // Send SIGCHLD to the parent process (if any)
        if let Some(parent_pid) = parent_pid {
            if let Some(parent_process) = self.processes.get_mut(&parent_pid) {
                use crate::signal::constants::SIGCHLD;
                parent_process.signals.set_pending(SIGCHLD);
                log::debug!(
                    "Sent SIGCHLD to parent process {} for child {} exit",
                    parent_pid.as_u64(),
                    pid.as_u64()
                );
            }
        }
    }

    /// Get the next ready process to run
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn all_pids(&self) -> Vec<ProcessId> {
        self.processes.keys().cloned().collect()
    }

    /// Get process count
    #[allow(dead_code)]
    pub fn process_count(&self) -> usize {
        self.processes.len()
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
        self.processes
            .iter()
            .find(|(_, process)| process.main_thread.as_ref().map(|t| t.id) == Some(thread_id))
            .map(|(pid, process)| (*pid, process))
    }

    /// Find a process by its main thread ID (mutable)
    pub fn find_process_by_thread_mut(
        &mut self,
        thread_id: u64,
    ) -> Option<(ProcessId, &mut Process)> {
        self.processes
            .iter_mut()
            .find(|(_, process)| process.main_thread.as_ref().map(|t| t.id) == Some(thread_id))
            .map(|(pid, process)| (*pid, process))
    }

    /// Find a process by its CR3 (page table frame)
    #[allow(dead_code)]
    pub fn find_process_by_cr3(&self, cr3: u64) -> Option<(ProcessId, &Process)> {
        self.processes
            .iter()
            .find(|(_, process)| {
                if let Some(ref pt) = process.page_table {
                    pt.level_4_frame().start_address().as_u64() == cr3
                } else {
                    false
                }
            })
            .map(|(pid, process)| (*pid, process))
    }

    /// Find a process by its CR3 (page table frame) - mutable version
    pub fn find_process_by_cr3_mut(&mut self, cr3: u64) -> Option<(ProcessId, &mut Process)> {
        log::trace!("find_process_by_cr3_mut: Looking for CR3={:#x}", cr3);

        let result = self.processes
            .iter_mut()
            .find(|(_, process)| {
                if let Some(ref pt) = process.page_table {
                    pt.level_4_frame().start_address().as_u64() == cr3
                } else {
                    false
                }
            })
            .map(|(pid, process)| (*pid, process));

        if let Some((pid, _)) = &result {
            log::trace!("Found: Process {} has CR3={:#x}", pid.as_u64(), cr3);
        } else {
            log::trace!("NOT FOUND: No process has CR3={:#x}", cr3);
        }

        result
    }

    /// Debug print all processes
    pub fn debug_processes(&self) {
        log::info!("=== Process List ===");
        for (pid, process) in &self.processes {
            log::info!(
                "  PID {}: {} - {:?}",
                pid.as_u64(),
                process.name,
                process.state
            );
        }
        log::info!("Current PID: {:?}", self.current_pid);
        log::info!("Ready queue: {:?}", self.ready_queue);
    }

    /// Get all processes (for contract testing)
    #[allow(dead_code)]
    pub fn all_processes(&self) -> Vec<&Process> {
        self.processes.values().collect()
    }

    /// Fork a process - create a child process that's a copy of the parent
    /// Note: Fork requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    pub fn fork_process(&mut self, parent_pid: ProcessId) -> Result<ProcessId, &'static str> {
        self.fork_process_with_context(parent_pid, None)
    }

    /// Fork a process with a pre-allocated page table
    /// This version accepts a page table created outside the lock to avoid deadlock
    /// Note: Fork requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)] // Part of public fork API - available for deadlock-free fork patterns
    pub fn fork_process_with_page_table(
        &mut self,
        parent_pid: ProcessId,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))] userspace_rsp: Option<u64>,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))] return_rip: Option<u64>,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables, unused_mut))] mut child_page_table: Box<ProcessPageTable>,
    ) -> Result<ProcessId, &'static str> {
        // Get the parent process info we need (including page table for memory copying)
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let (parent_name, parent_entry_point, parent_pgid, parent_sid, parent_cwd, parent_thread_info, parent_heap_start, parent_heap_end, parent_mmap_hint, parent_vmas) = {
            let parent = self
                .processes
                .get(&parent_pid)
                .ok_or("Parent process not found")?;

            let _parent_thread = parent
                .main_thread
                .as_ref()
                .ok_or("Parent process has no main thread")?;

            // Clone what we need to avoid borrow issues
            (
                parent.name.clone(),
                parent.entry_point,
                parent.pgid,
                parent.sid,
                parent.cwd.clone(),
                _parent_thread.clone(),
                parent.heap_start,
                parent.heap_end,
                parent.mmap_hint,
                parent.vmas.clone(),
            )
        };

        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));

        log::info!(
            "Forking process {} '{}' -> child PID {}",
            parent_pid.as_u64(),
            parent_name,
            child_pid.as_u64()
        );

        // Create child process name
        let child_name = format!("{}_child_{}", parent_name, child_pid.as_u64());

        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent_entry_point);
        child_process.parent = Some(parent_pid);
        // POSIX: Child inherits parent's process group, session, and working directory
        child_process.pgid = parent_pgid;
        child_process.sid = parent_sid;
        child_process.cwd = parent_cwd.clone();

        // COPY-ON-WRITE FORK: Share pages between parent and child
        #[cfg(feature = "testing")]
        {
            // Get mutable access to parent's page table for CoW setup
            let parent = self
                .processes
                .get_mut(&parent_pid)
                .ok_or("Parent process not found during CoW setup")?;
            let mut parent_page_table = parent
                .page_table
                .take()
                .ok_or("Parent process has no page table")?;

            // Set up Copy-on-Write sharing between parent and child
            let pages_shared = super::fork::setup_cow_pages(
                parent_page_table.as_mut(),
                child_page_table.as_mut(),
            )?;

            // Put parent's page table back
            parent.page_table = Some(parent_page_table);

            log::info!(
                "fork_process_with_page_table: Set up {} pages for CoW sharing",
                pages_shared
            );

            // Child inherits parent's heap bounds and mmap state
            child_process.heap_start = parent_heap_start;
            child_process.heap_end = parent_heap_end;
            child_process.mmap_hint = parent_mmap_hint;
            child_process.vmas = parent_vmas;
        }
        #[cfg(not(feature = "testing"))]
        {
            log::error!("fork_process: Cannot fork - testing feature not enabled");
            return Err("Cannot implement fork without testing feature");
        }

        #[cfg(feature = "testing")]
        {
            child_process.page_table = Some(child_page_table);

            // Continue with the rest of the fork logic...
            self.complete_fork(
                parent_pid,
                child_pid,
                &parent_thread_info,
                userspace_rsp,
                return_rip,
                None, // No parent context - will use parent_thread.context
                child_process,
            )
        }
    }

    /// Fork a process with the ACTUAL parent register state from syscall frame
    /// This is the preferred method as it captures the exact register values at fork time,
    /// not the stale values from the last context switch.
    /// Note: Fork requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    pub fn fork_process_with_parent_context(
        &mut self,
        parent_pid: ProcessId,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        parent_context: crate::task::thread::CpuContext,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables, unused_mut))]
        mut child_page_table: Box<ProcessPageTable>,
    ) -> Result<ProcessId, &'static str> {
        // Get the parent process info we need (including page table for memory copying)
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let (parent_name, parent_entry_point, parent_pgid, parent_sid, parent_cwd, parent_thread_info, parent_heap_start, parent_heap_end, parent_mmap_hint, parent_vmas) = {
            let parent = self
                .processes
                .get(&parent_pid)
                .ok_or("Parent process not found")?;

            let _parent_thread = parent
                .main_thread
                .as_ref()
                .ok_or("Parent process has no main thread")?;

            // Clone what we need to avoid borrow issues
            (
                parent.name.clone(),
                parent.entry_point,
                parent.pgid,
                parent.sid,
                parent.cwd.clone(),
                _parent_thread.clone(),
                parent.heap_start,
                parent.heap_end,
                parent.mmap_hint,
                parent.vmas.clone(),
            )
        };

        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));

        log::info!(
            "Forking process {} '{}' -> child PID {}",
            parent_pid.as_u64(),
            parent_name,
            child_pid.as_u64()
        );

        // Create child process name
        let child_name = format!("{}_child_{}", parent_name, child_pid.as_u64());

        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent_entry_point);
        child_process.parent = Some(parent_pid);
        // POSIX: Child inherits parent's process group, session, and working directory
        child_process.pgid = parent_pgid;
        child_process.sid = parent_sid;
        child_process.cwd = parent_cwd.clone();

        // COPY-ON-WRITE FORK: Share pages between parent and child
        // Pages are marked read-only and only copied when written to
        #[cfg(feature = "testing")]
        {
            // Get mutable access to parent's page table for CoW setup
            // We temporarily take ownership to modify parent's page flags
            let parent = self
                .processes
                .get_mut(&parent_pid)
                .ok_or("Parent process not found during CoW setup")?;
            let mut parent_page_table = parent
                .page_table
                .take()
                .ok_or("Parent process has no page table")?;

            // Set up Copy-on-Write sharing between parent and child
            let pages_shared = super::fork::setup_cow_pages(
                parent_page_table.as_mut(),
                child_page_table.as_mut(),
            )?;

            // Put parent's page table back
            parent.page_table = Some(parent_page_table);

            log::info!(
                "fork_process: Set up {} pages for CoW sharing",
                pages_shared
            );

            // Child inherits parent's heap bounds and mmap state
            child_process.heap_start = parent_heap_start;
            child_process.heap_end = parent_heap_end;
            child_process.mmap_hint = parent_mmap_hint;
            child_process.vmas = parent_vmas;
        }
        #[cfg(not(feature = "testing"))]
        {
            log::error!("fork_process: Cannot fork - testing feature not enabled");
            return Err("Cannot implement fork without testing feature");
        }

        #[cfg(feature = "testing")]
        {
            child_process.page_table = Some(child_page_table);

            // Use the actual parent context from the syscall frame
            self.complete_fork(
                parent_pid,
                child_pid,
                &parent_thread_info,
                Some(parent_context.rsp),
                Some(parent_context.rip),
                Some(parent_context), // Pass the actual parent context
                child_process,
            )
        }
    }

    /// Fork a process on ARM64 with the ACTUAL parent register state from exception frame
    ///
    /// This is the ARM64 equivalent of fork_process_with_parent_context. It captures the
    /// exact register values at fork time from the exception frame.
    #[cfg(target_arch = "aarch64")]
    pub fn fork_process_aarch64(
        &mut self,
        parent_pid: ProcessId,
        parent_context: crate::task::thread::CpuContext,
        mut child_page_table: Box<ProcessPageTable>,
    ) -> Result<ProcessId, &'static str> {
        // Lock-free trace: fork entry
        crate::tracing::providers::process::trace_fork_entry(parent_pid.as_u64() as u32);

        // Get the parent process info
        let (parent_name, parent_entry_point, parent_pgid, parent_sid, parent_cwd, parent_thread_info, parent_heap_start, parent_heap_end, parent_mmap_hint, parent_vmas) = {
            let parent = self
                .processes
                .get(&parent_pid)
                .ok_or("Parent process not found")?;

            let parent_thread = parent
                .main_thread
                .as_ref()
                .ok_or("Parent process has no main thread")?;

            (
                parent.name.clone(),
                parent.entry_point,
                parent.pgid,
                parent.sid,
                parent.cwd.clone(),
                parent_thread.clone(),
                parent.heap_start,
                parent.heap_end,
                parent.mmap_hint,
                parent.vmas.clone(),
            )
        };

        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));

        log::info!(
            "ARM64 fork: process {} '{}' -> child PID {}",
            parent_pid.as_u64(),
            parent_name,
            child_pid.as_u64()
        );

        // Create child process name
        let child_name = format!("{}_child_{}", parent_name, child_pid.as_u64());

        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent_entry_point);
        child_process.parent = Some(parent_pid);
        // POSIX: Child inherits parent's process group, session, and working directory
        child_process.pgid = parent_pgid;
        child_process.sid = parent_sid;
        child_process.cwd = parent_cwd.clone();

        // COPY-ON-WRITE FORK: Share pages between parent and child
        {
            // Get mutable access to parent's page table for CoW setup
            let parent = self
                .processes
                .get_mut(&parent_pid)
                .ok_or("Parent process not found during CoW setup")?;
            let mut parent_page_table = parent
                .page_table
                .take()
                .ok_or("Parent process has no page table")?;

            // Set up Copy-on-Write sharing between parent and child
            let pages_shared = super::fork::setup_cow_pages(
                parent_page_table.as_mut(),
                child_page_table.as_mut(),
            )?;

            // Put parent's page table back
            parent.page_table = Some(parent_page_table);

            log::info!(
                "ARM64 fork: Set up {} pages for CoW sharing",
                pages_shared
            );

            // Child inherits parent's heap bounds
            child_process.heap_start = parent_heap_start;
            child_process.heap_end = parent_heap_end;

            // Child inherits parent's mmap state so it doesn't collide with
            // CoW-shared pages that are already mapped in the child's page table
            child_process.mmap_hint = parent_mmap_hint;
            child_process.vmas = parent_vmas;
        }

        child_process.page_table = Some(child_page_table);

        // Complete the fork with ARM64-specific handling
        self.complete_fork_aarch64(
            parent_pid,
            child_pid,
            &parent_thread_info,
            parent_context,
            child_process,
        )
    }

    /// Complete the fork operation for ARM64 after page table is created
    ///
    /// ARM64 key differences from x86_64:
    /// - SP_EL0 holds user stack pointer (not RSP)
    /// - ELR_EL1 holds return address (not RIP)
    /// - X0 is the return value register (not RAX)
    /// - 16-byte stack alignment required
    #[cfg(target_arch = "aarch64")]
    fn complete_fork_aarch64(
        &mut self,
        parent_pid: ProcessId,
        child_pid: ProcessId,
        parent_thread: &Thread,
        parent_context: crate::task::thread::CpuContext,
        mut child_process: Process,
    ) -> Result<ProcessId, &'static str> {
        use crate::memory::arch_stub::VirtAddr;

        log::info!(
            "ARM64 complete_fork: Creating child thread for PID {}",
            child_pid.as_u64()
        );

        // For fork, the child inherits the parent's address space layout.
        // Use the parent's user stack virtual addresses - map_user_stack_to_process
        // will allocate new physical frames and map them to these addresses.
        let child_stack_top = parent_thread.stack_top;
        let child_stack_bottom = parent_thread.stack_bottom;

        log::debug!(
            "ARM64 fork: Using parent's user stack range {:#x}-{:#x}",
            child_stack_bottom.as_u64(),
            child_stack_top.as_u64()
        );

        // Map the stack pages into the child's page table
        // This allocates new physical frames and maps them to the user addresses
        let child_page_table_ref = child_process
            .page_table
            .as_mut()
            .ok_or("Child process has no page table")?;

        crate::memory::process_memory::map_user_stack_to_process(
            child_page_table_ref,
            child_stack_bottom,
            child_stack_top,
        )
        .map_err(|e| {
            log::error!("ARM64 fork: Failed to map user stack: {}", e);
            "Failed to map user stack in child's page table"
        })?;
        crate::tracing::providers::process::trace_stack_map(child_pid.as_u64() as u32);

        // Allocate a globally unique thread ID for the child's main thread
        let child_thread_id = crate::task::thread::allocate_thread_id();

        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);

        // Allocate a kernel stack for the child thread
        let child_kernel_stack_top = if parent_thread.privilege == crate::task::thread::ThreadPrivilege::User {
            let kernel_stack = crate::memory::kernel_stack::allocate_kernel_stack().map_err(|e| {
                log::error!("ARM64 fork: Failed to allocate kernel stack: {}", e);
                "Failed to allocate kernel stack for child thread"
            })?;
            let kernel_stack_top = kernel_stack.top();

            log::debug!(
                "ARM64 fork: Allocated child kernel stack at {:#x}",
                kernel_stack_top.as_u64()
            );

            // Store the kernel stack (leak for now - TODO: proper cleanup)
            Box::leak(Box::new(kernel_stack));

            kernel_stack_top
        } else {
            parent_thread.kernel_stack_top.unwrap_or(parent_thread.stack_top)
        };

        // Create the child's main thread
        fn dummy_entry() {}

        let mut child_thread = Thread::new(
            format!("{}_main", child_process.name),
            dummy_entry,
            child_stack_top,
            parent_thread.stack_bottom,
            child_tls_block,
            parent_thread.privilege,
        );

        // Set the ID and kernel stack
        child_thread.id = child_thread_id;
        child_thread.kernel_stack_top = Some(child_kernel_stack_top);

        // Copy parent's thread context from the exception frame
        log::debug!("ARM64 fork: Copying parent context to child");
        log::debug!(
            "  Parent: SP_EL0={:#x}, ELR_EL1={:#x}, SPSR={:#x}",
            parent_context.sp_el0, parent_context.elr_el1, parent_context.spsr_el1
        );

        child_thread.context = parent_context.clone();

        // CRITICAL: Fork returns 0 to child. The parent_context captured x0 at
        // SVC entry time, which is undefined (ARM64 syscall0 uses lateout("x0")).
        // Without this, the child sees a random x0 and doesn't enter the child branch.
        child_thread.context.x0 = 0;

        // CRITICAL: Set has_started=true for forked children
        child_thread.has_started = true;

        // Calculate the child's stack pointer based on parent's stack usage
        let parent_sp = parent_context.sp_el0;
        let parent_stack_used = parent_thread.stack_top.as_u64().saturating_sub(parent_sp);
        let child_sp = child_stack_top.as_u64().saturating_sub(parent_stack_used);
        // ARM64 requires 16-byte alignment
        let child_sp_aligned = child_sp & !0xF;
        child_thread.context.sp_el0 = child_sp_aligned;

        log::info!(
            "ARM64 fork: parent_stack_top={:#x}, parent_sp={:#x}, used={:#x}",
            parent_thread.stack_top.as_u64(),
            parent_sp,
            parent_stack_used
        );
        log::info!(
            "ARM64 fork: child_stack_top={:#x}, child_sp={:#x}",
            child_stack_top.as_u64(),
            child_sp_aligned
        );

        // Copy the parent's stack contents to the child's stack using HHDM-based physical access.
        // CRITICAL: We cannot use virtual addresses directly because TTBR0 still points to the
        // parent's page table. Writing to child virtual addresses would actually write to parent
        // physical frames. Instead, we must:
        // 1. Translate parent VA -> parent PA using parent's page table
        // 2. Translate child VA -> child PA using child's page table
        // 3. Copy using HHDM addresses: (HHDM_BASE + phys_addr)
        if parent_stack_used > 0 && parent_stack_used <= (64 * 1024) {
            log::debug!(
                "ARM64 fork: Copying {} bytes of stack from {:#x} to {:#x} via HHDM",
                parent_stack_used,
                parent_sp,
                child_sp_aligned
            );

            let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;

            // Get parent's page table for address translation
            let parent_page_table = self
                .processes
                .get(&parent_pid)
                .and_then(|p| p.page_table.as_ref())
                .ok_or("Parent process page table not found for stack copy")?;

            // Get child's page table for address translation
            let child_page_table = child_process
                .page_table
                .as_ref()
                .ok_or("Child process page table not found for stack copy")?;

            // Copy stack page by page
            // Start from the page containing SP (page-align down)
            let start_page_addr = parent_sp & !0xFFF;
            let mut parent_page_addr = start_page_addr;
            let parent_stack_top_u64 = parent_thread.stack_top.as_u64();
            let child_stack_top_u64 = child_stack_top.as_u64();

            while parent_page_addr < parent_stack_top_u64 {
                // Calculate corresponding child page address (same offset from stack top)
                let offset_from_top = parent_stack_top_u64 - parent_page_addr;
                let child_page_addr = child_stack_top_u64 - offset_from_top;

                // Translate parent virtual address to physical
                let parent_phys = match parent_page_table.translate_page(VirtAddr::new(parent_page_addr)) {
                    Some(phys) => phys,
                    None => {
                        log::warn!(
                            "ARM64 fork: parent stack page {:#x} not mapped, skipping",
                            parent_page_addr
                        );
                        parent_page_addr += 4096;
                        continue;
                    }
                };

                // Translate child virtual address to physical
                let child_phys = match child_page_table.translate_page(VirtAddr::new(child_page_addr)) {
                    Some(phys) => phys,
                    None => {
                        log::error!(
                            "ARM64 fork: child stack page {:#x} not mapped!",
                            child_page_addr
                        );
                        return Err("Child stack page not mapped");
                    }
                };

                // Copy via HHDM (kernel's direct physical memory mapping)
                let parent_hhdm_addr = (hhdm_base + parent_phys.as_u64()) as *const u8;
                let child_hhdm_addr = (hhdm_base + child_phys.as_u64()) as *mut u8;

                unsafe {
                    core::ptr::copy_nonoverlapping(parent_hhdm_addr, child_hhdm_addr, 4096);
                }

                log::trace!(
                    "ARM64 fork: copied stack page {:#x} (phys {:#x}) -> {:#x} (phys {:#x})",
                    parent_page_addr,
                    parent_phys.as_u64(),
                    child_page_addr,
                    child_phys.as_u64()
                );

                parent_page_addr += 4096;
            }

            log::info!(
                "ARM64 fork: Copied stack pages from parent to child via HHDM"
            );
        }

        // Set the kernel stack pointer for exception handling
        child_thread.context.sp = child_kernel_stack_top.as_u64();

        // CRUCIAL: Set the child's return value to 0 in X0
        // On ARM64, X0 is the return value register (like RAX on x86_64)
        // The child process must receive 0 from fork(), while the parent gets child_pid
        child_thread.context.x0 = 0;

        log::info!(
            "ARM64 fork: Created child thread {} with ELR={:#x}, SP_EL0={:#x}, X0={}",
            child_thread_id,
            child_thread.context.elr_el1,
            child_thread.context.sp_el0,
            child_thread.context.x0
        );

        // Set the child process's main thread
        child_process.main_thread = Some(child_thread);


        // Copy all other process state (fd_table, signals, verify pgid/sid)
        if let Some(parent) = self.processes.get(&parent_pid) {
            if let Err(e) = super::fork::copy_process_state(parent, &mut child_process) {
                log::error!(
                    "ARM64 fork: Failed to copy process state: {}",
                    e
                );
                return Err(e);
            }
        } else {
            log::error!(
                "ARM64 fork: Parent {} not found when copying process state!",
                parent_pid.as_u64()
            );
            return Err("Parent process not found during state copy");
        }

        // Add the child to the parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.children.push(child_pid);
        }

        // Insert the child process into the process table
        self.processes.insert(child_pid, child_process);

        log::info!(
            "ARM64 fork complete: parent {} -> child {}",
            parent_pid.as_u64(),
            child_pid.as_u64()
        );

        // Lock-free trace: fork exit with child PID
        crate::tracing::providers::process::trace_fork_exit(child_pid.as_u64() as u32);

        Ok(child_pid)
    }

    /// Complete the fork operation after page table is created
    /// If `parent_context_override` is provided, it will be used for the child's context
    /// instead of the stale values from `parent_thread.context`.
    /// Note: Uses architecture-specific register names
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    fn complete_fork(
        &mut self,
        parent_pid: ProcessId,
        child_pid: ProcessId,
        parent_thread: &Thread,
        userspace_rsp: Option<u64>,
        return_rip: Option<u64>,
        parent_context_override: Option<crate::task::thread::CpuContext>,
        mut child_process: Process,
    ) -> Result<ProcessId, &'static str> {
        log::info!(
            "Created page table for child process {}",
            child_pid.as_u64()
        );

        // Create a new stack for the child process (64KB userspace stack)
        // CRITICAL: We allocate in kernel page table first, then map to child's page table
        const CHILD_STACK_SIZE: usize = 64 * 1024;

        // Allocate the stack in the kernel page table first
        let child_stack = crate::memory::stack::allocate_stack_with_privilege(
            CHILD_STACK_SIZE,
            crate::task::thread::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to allocate stack for child process")?;
        let child_stack_top = child_stack.top();
        let child_stack_bottom = child_stack.bottom();

        // Now map the stack pages into the child's page table
        let child_page_table_ref = child_process
            .page_table
            .as_mut()
            .ok_or("Child process has no page table")?;

        crate::memory::process_memory::map_user_stack_to_process(
            child_page_table_ref,
            child_stack_bottom,
            child_stack_top,
        )
        .map_err(|e| {
            log::error!("Failed to map user stack to child process: {}", e);
            "Failed to map user stack in child's page table"
        })?;

        // For now, use a dummy TLS address - the Thread constructor will allocate proper TLS
        // In the future, we should properly copy parent's TLS data
        let _dummy_tls = VirtAddr::new(0);

        // Allocate a globally unique thread ID for the child's main thread
        // NOTE: While Unix convention is TID = PID for main thread, we need global
        // uniqueness across all threads (kernel + user).
        let child_thread_id = crate::task::thread::allocate_thread_id();

        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);

        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
            log::warn!(
                "Failed to register thread {} with TLS system: {}",
                child_thread_id,
                e
            );
        }

        // Allocate a kernel stack for the child thread (userspace threads need kernel stacks)
        let child_kernel_stack_top =
            if parent_thread.privilege == crate::task::thread::ThreadPrivilege::User {
                // Use the new global kernel stack allocator
                let kernel_stack =
                    crate::memory::kernel_stack::allocate_kernel_stack().map_err(|e| {
                        log::error!("Failed to allocate kernel stack for child: {}", e);
                        "Failed to allocate kernel stack for child thread"
                    })?;
                let kernel_stack_top = kernel_stack.top();

                log::debug!(
                    "✓ Allocated child kernel stack at {:#x} (globally visible)",
                    kernel_stack_top
                );

                // Store the kernel stack (we'll need to manage this properly later)
                // For now, we'll leak it - TODO: proper cleanup
                Box::leak(Box::new(kernel_stack));

                kernel_stack_top
            } else {
                // Kernel threads don't need separate kernel stacks
                parent_thread
                    .kernel_stack_top
                    .unwrap_or(parent_thread.stack_top)
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
        // CRITICAL: Use parent_context_override if provided - this contains the ACTUAL
        // register values from the syscall frame, not the stale values from last context switch
        child_thread.context = match parent_context_override {
            Some(ctx) => {
                log::debug!("fork: Using actual parent context from syscall frame");
                log::debug!(
                    "  Parent registers: rbx={:#x}, r12={:#x}, r13={:#x}, r14={:#x}, r15={:#x}",
                    ctx.rbx, ctx.r12, ctx.r13, ctx.r14, ctx.r15
                );
                ctx
            }
            None => {
                log::debug!("fork: Using stale parent_thread.context (may have incorrect register values!)");
                parent_thread.context.clone()
            }
        };

        // CRITICAL: Set has_started=true for forked children so they use the
        // restore_userspace_context path which preserves the cloned register values
        // instead of the first-run path which zeros all registers.
        child_thread.has_started = true;

        // Log the child's context for debugging
        log::debug!("Child thread context after copy:");
        log::debug!("  RIP: {:#x}", child_thread.context.rip);
        log::debug!("  RSP: {:#x}", child_thread.context.rsp);
        log::debug!("  CS: {:#x}", child_thread.context.cs);
        log::debug!("  SS: {:#x}", child_thread.context.ss);

        // Crucial: Set the child's return value to 0
        // In x86_64, system call return values go in RAX
        child_thread.context.rax = 0;

        // Calculate the child's stack pointer based on parent's stack usage
        // CRITICAL: We MUST calculate RSP relative to the child's stack, not use parent's RSP directly!
        // The parent's RSP points into parent's stack address space, but the child has its own stack.
        let parent_rsp = userspace_rsp.unwrap_or(parent_thread.context.rsp);
        let parent_stack_used = parent_thread.stack_top.as_u64().saturating_sub(parent_rsp);
        let child_rsp = child_stack_top.as_u64().saturating_sub(parent_stack_used);
        child_thread.context.rsp = child_rsp;
        log::info!(
            "fork: parent_stack_top={:#x}, parent_rsp={:#x}, used={:#x}",
            parent_thread.stack_top.as_u64(),
            parent_rsp,
            parent_stack_used
        );
        log::info!(
            "fork: child_stack_top={:#x}, child_rsp={:#x}",
            child_stack_top.as_u64(),
            child_rsp
        );

        // CRITICAL: Copy the parent's stack contents to the child's stack
        // This ensures local variables (like `write_fd`, `pipefd`, etc.) are preserved
        if parent_stack_used > 0 && parent_stack_used <= (64 * 1024) {
            // Access parent's stack through the kernel page table
            // Both parent and child stacks are identity-mapped in kernel space
            let parent_stack_src = parent_rsp as *const u8;
            let child_stack_dst = child_rsp as *mut u8;

            // Debug: Show first few words of parent's stack before copy
            log::debug!(
                "fork: Stack copy debug - parent_src={:#x}, child_dst={:#x}, bytes={}",
                parent_rsp,
                child_rsp,
                parent_stack_used
            );
            unsafe {
                let parent_words = parent_stack_src as *const u64;
                for i in 0..core::cmp::min(16, (parent_stack_used / 8) as isize) {
                    let val = *parent_words.offset(i);
                    if val != 0 {
                        log::debug!("  parent stack[{}]: {:#x}", i, val);
                    }
                }
            }

            unsafe {
                core::ptr::copy_nonoverlapping(
                    parent_stack_src,
                    child_stack_dst,
                    parent_stack_used as usize,
                );
            }

            // Debug: Verify copy by checking first few words of child's stack
            unsafe {
                let child_words = child_stack_dst as *const u64;
                for i in 0..core::cmp::min(16, (parent_stack_used / 8) as isize) {
                    let val = *child_words.offset(i);
                    if val != 0 {
                        log::debug!("  child stack[{}]: {:#x}", i, val);
                    }
                }
            }

            log::info!(
                "fork: Copied {} bytes of stack from parent to child",
                parent_stack_used
            );
        }

        // Update child's instruction pointer to return to the instruction after fork syscall
        // The return RIP comes from RCX which was saved by the syscall instruction
        if let Some(rip) = return_rip {
            child_thread.context.rip = rip;
            log::info!("fork: Using return RIP {:#x} for child", rip);
        } else {
            // If no return RIP provided, keep the parent's RIP (fallback for sys_fork without frame)
            log::info!(
                "fork: No return RIP provided, using parent RIP {:#x}",
                child_thread.context.rip
            );
        }

        log::info!(
            "Created child thread {} with entry point {:#x}",
            child_thread_id,
            child_process.entry_point
        );

        // Set the child process's main thread
        child_process.main_thread = Some(child_thread);

        // Store the stack in the child process
        child_process.stack = Some(Box::new(child_stack));

        // Copy all other process state (fd_table, signals, verify pgid/sid)
        // This uses the centralized copy_process_state which handles:
        // - File descriptor table cloning (with proper pipe refcount handling)
        // - Signal state forking (handlers and mask, NOT pending signals)
        // - Verification of pgid and sid inheritance
        if let Some(parent) = self.processes.get(&parent_pid) {
            if let Err(e) = super::fork::copy_process_state(parent, &mut child_process) {
                log::error!(
                    "fork: Failed to copy process state from parent {} to child {}: {}",
                    parent_pid.as_u64(),
                    child_pid.as_u64(),
                    e
                );
                return Err(e);
            }
        } else {
            log::error!(
                "fork: CRITICAL - Parent {} not found when copying process state!",
                parent_pid.as_u64()
            );
            return Err("Parent process not found during state copy");
        }

        // Add the child to the parent's children list
        if let Some(parent) = self.processes.get_mut(&parent_pid) {
            parent.children.push(child_pid);
        }

        // Insert the child process into the process table
        log::debug!("About to insert child process into process table");
        self.processes.insert(child_pid, child_process);
        log::debug!("Child process inserted successfully");

        log::info!(
            "Fork complete: parent {} -> child {}",
            parent_pid.as_u64(),
            child_pid.as_u64()
        );

        Ok(child_pid)
    }

    /// Fork a process with optional userspace context override
    /// NOTE: This method creates the page table while holding the lock, which can cause deadlock
    /// Consider using fork_process_with_page_table instead
    /// Note: Fork requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    pub fn fork_process_with_context(
        &mut self,
        parent_pid: ProcessId,
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        userspace_rsp: Option<u64>,
    ) -> Result<ProcessId, &'static str> {
        // Get the parent process
        let parent = self
            .processes
            .get(&parent_pid)
            .ok_or("Parent process not found")?;

        // Get parent's main thread (used in testing builds for context cloning)
        // Clone to avoid borrow issues when we need mutable access to parent later
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let parent_thread = parent
            .main_thread
            .as_ref()
            .ok_or("Parent process has no main thread")?
            .clone();

        // Allocate a new PID for the child
        let child_pid = ProcessId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));

        log::info!(
            "Forking process {} '{}' -> child PID {}",
            parent_pid.as_u64(),
            parent.name,
            child_pid.as_u64()
        );

        // Create child process name
        let child_name = format!("{}_child_{}", parent.name, child_pid.as_u64());

        // Capture parent's pgid, sid, and cwd before borrowing page_table
        let parent_pgid = parent.pgid;
        let parent_sid = parent.sid;
        let parent_cwd = parent.cwd.clone();

        // Create the child process with the same entry point
        let mut child_process = Process::new(child_pid, child_name.clone(), parent.entry_point);
        child_process.parent = Some(parent_pid);
        // POSIX: Child inherits parent's process group, session, and working directory
        child_process.pgid = parent_pgid;
        child_process.sid = parent_sid;
        child_process.cwd = parent_cwd;

        // Extract parent heap/mmap bounds before we drop the parent borrow
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let parent_heap_start = parent.heap_start;
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let parent_heap_end = parent.heap_end;
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let parent_mmap_hint = parent.mmap_hint;
        #[cfg_attr(not(feature = "testing"), allow(unused_variables))]
        let parent_vmas = parent.vmas.clone();

        // Verify parent has a page table
        if parent.page_table.is_none() {
            return Err("Parent process has no page table");
        }

        // Create a new page table for the child process
        log::debug!("fork_process: About to create child page table");
        let child_page_table_result = crate::memory::process_memory::ProcessPageTable::new();
        log::debug!("fork_process: ProcessPageTable::new() returned");
        #[cfg_attr(not(feature = "testing"), allow(unused_mut, unused_variables))]
        let mut child_page_table =
            Box::new(child_page_table_result.map_err(|_| "Failed to create child page table")?);
        log::debug!("fork_process: Child page table created successfully");

        // COPY-ON-WRITE FORK: Share pages between parent and child
        #[cfg(feature = "testing")]
        {
            // Get mutable access to parent's page table for CoW setup
            let parent_mut = self
                .processes
                .get_mut(&parent_pid)
                .ok_or("Parent process not found during CoW setup")?;
            let mut parent_page_table = parent_mut
                .page_table
                .take()
                .ok_or("Parent process has no page table")?;

            // Log page table addresses for debugging
            log::debug!(
                "Parent page table CR3: {:#x}",
                parent_page_table.level_4_frame().start_address()
            );
            log::debug!(
                "Child page table CR3: {:#x}",
                child_page_table.level_4_frame().start_address()
            );

            // Set up Copy-on-Write sharing between parent and child
            let pages_shared = super::fork::setup_cow_pages(
                parent_page_table.as_mut(),
                child_page_table.as_mut(),
            )?;

            // Put parent's page table back
            parent_mut.page_table = Some(parent_page_table);

            log::info!(
                "fork_process_with_context: Set up {} pages for CoW sharing",
                pages_shared
            );

            // Child inherits parent's heap bounds and mmap state
            child_process.heap_start = parent_heap_start;
            child_process.heap_end = parent_heap_end;
            child_process.mmap_hint = parent_mmap_hint;
            child_process.vmas = parent_vmas;
        }
        #[cfg(not(feature = "testing"))]
        {
            log::error!("fork_process: Cannot fork - testing feature not enabled");
            return Err("Cannot implement fork without testing feature");
        }

        #[cfg(feature = "testing")]
        {
            child_process.page_table = Some(child_page_table);

            log::info!(
                "Created page table for child process {}",
                child_pid.as_u64()
            );
        }

        #[cfg(feature = "testing")]
        {
            // Create a new stack for the child process (64KB userspace stack)
            const CHILD_STACK_SIZE: usize = 64 * 1024;
            let child_stack = crate::memory::stack::allocate_stack_with_privilege(
            CHILD_STACK_SIZE,
            crate::task::thread::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to allocate stack for child process")?;
        let child_stack_top = child_stack.top();

        // For now, use a dummy TLS address - the Thread constructor will allocate proper TLS
        // In the future, we should properly copy parent's TLS data
        let _dummy_tls = VirtAddr::new(0);

        // Allocate a globally unique thread ID for the child's main thread
        // NOTE: While Unix convention is TID = PID for main thread, we need global
        // uniqueness across all threads (kernel + user).
        let child_thread_id = crate::task::thread::allocate_thread_id();

        // Allocate a TLS block for this thread ID
        let child_tls_block = VirtAddr::new(0x10000 + child_thread_id * 0x1000);

        // Register this thread with the TLS system
        if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
            log::warn!(
                "Failed to register thread {} with TLS system: {}",
                child_thread_id,
                e
            );
        }

        // Allocate a kernel stack for the child thread (userspace threads need kernel stacks)
        let child_kernel_stack_top =
            if parent_thread.privilege == crate::task::thread::ThreadPrivilege::User {
                const KERNEL_STACK_SIZE: usize = 16 * 1024; // 16KB kernel stack
                let kernel_stack = crate::memory::stack::allocate_stack_with_privilege(
                    KERNEL_STACK_SIZE,
                    crate::task::thread::ThreadPrivilege::Kernel,
                )
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
            stack_top: child_stack_top,
            stack_bottom: child_stack_top - (64 * 1024),
            kernel_stack_top: child_kernel_stack_top,
            kernel_stack_allocation: None, // Kernel stack for userspace thread not managed here
            tls_block: child_tls_block,
            priority: parent_thread.priority,
            time_slice: parent_thread.time_slice,
            entry_point: None, // Userspace threads don't have kernel entry points
            privilege: parent_thread.privilege,
            // CRITICAL: Set has_started=true for forked children so they use the
            // restore_userspace_context path which preserves the cloned register values
            // instead of the first-run path which zeros all registers.
            // The child should resume from the same context as the parent.
            has_started: true,
            blocked_in_syscall: false, // Forked child is not blocked in syscall
            saved_userspace_context: None, // Child starts fresh
            wake_time_ns: None,
        };

        // CRITICAL: Use the userspace RSP if provided (from syscall frame)
        // Otherwise, calculate the child's RSP based on parent's stack usage
        if let Some(user_rsp) = userspace_rsp {
            child_thread.context.rsp = user_rsp;
            log::info!("fork: Using userspace RSP {:#x} for child", user_rsp);
        } else {
            // Calculate how much of parent's stack is in use
            let parent_stack_used = parent_thread.stack_top.as_u64() - parent_thread.context.rsp;
            // Set child's RSP at the same relative position
            child_thread.context.rsp = child_stack_top.as_u64() - parent_stack_used;
            log::info!(
                "fork: Calculated child RSP {:#x} based on parent stack usage",
                child_thread.context.rsp
            );
        }

        // IMPORTANT: Set RAX to 0 for the child (fork return value)
        child_thread.context.rax = 0;

        // Set up child thread properties
        child_thread.privilege = parent_thread.privilege;
        // Mark child as ready to run
        child_thread.state = crate::task::thread::ThreadState::Ready;

        // Store the stack in the child process
        child_process.stack = Some(Box::new(child_stack));

        // Copy stack contents from parent to child
        // Note: User pages were set up for CoW sharing earlier with setup_cow_pages().
        // However, the child has a new stack at a different virtual address,
        // so we need to copy the stack contents separately.
        // Re-acquire parent references (we dropped them earlier for CoW setup)
        let parent = self
            .processes
            .get(&parent_pid)
            .ok_or("Parent process not found for stack copy")?;
        let parent_page_table = parent
            .page_table
            .as_ref()
            .ok_or("Parent process has no page table for stack copy")?;
        let child_page_table_ref = child_process
            .page_table
            .as_ref()
            .ok_or("Child process has no page table")?;
        super::fork::copy_stack_contents(
            &parent_thread,
            &mut child_thread,
            parent_page_table,
            child_page_table_ref,
        )?;
        // Copy all other process state (fd_table, signals, verify pgid/sid)
        // This is done via copy_process_state which handles:
        // - File descriptor table cloning (with proper pipe refcount handling)
        // - Signal state forking (handlers and mask, NOT pending signals)
        // - Verification of pgid and sid inheritance
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
        if let Some(kernel_stack_top) = child_kernel_stack_top {
            log::debug!(
                "Child kernel stack at {:#x} is globally visible via shared kernel PDPT",
                kernel_stack_top.as_u64()
            );
        }

        // Add the child to the ready queue so it can be scheduled
        self.ready_queue.push(child_pid);

        log::info!(
            "Fork complete: parent {} -> child {}",
            parent_pid.as_u64(),
            child_pid.as_u64()
        );

            // Return the child PID to the parent
            Ok(child_pid)
        } // End of #[cfg(feature = "testing")] block
    }

    /// Replace a process's address space with a new program (exec)
    ///
    /// This implements the exec() family of system calls. Unlike fork(), which creates
    /// a new process, exec() replaces the current process's address space with a new
    /// program while keeping the same PID.
    ///
    /// The `program_name` parameter is optional - if provided, it updates the process name
    /// to match the new program. This is critical because fork() uses the process name to
    /// reload the binary from disk.
    /// Note: Exec requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    pub fn exec_process(&mut self, pid: ProcessId, elf_data: &[u8], program_name: Option<&str>) -> Result<u64, &'static str> {
        log::info!(
            "exec_process: Replacing process {} with new program",
            pid.as_u64()
        );

        // CRITICAL OS-STANDARD CHECK: Is this the current process?
        let is_current_process = self.current_pid == Some(pid);
        if is_current_process {
            log::info!("exec_process: Executing on current process - special handling required");
        }

        // Get the existing process
        let process = self.processes.get_mut(&pid).ok_or("Process not found")?;

        // Drain any pending old page tables from previous exec() calls.
        // By this point, CR3 has definitely switched away from any old tables.
        process.drain_old_page_tables();

        // For now, assume non-current processes are not actively running
        // This is a simplification - in a real OS we'd check the scheduler state
        let is_scheduled = false;

        // Get the main thread (we need to preserve its ID)
        let main_thread = process
            .main_thread
            .as_ref()
            .ok_or("Process has no main thread")?;
        let thread_id = main_thread.id;
        let _old_stack_top = main_thread.stack_top;

        // Store old page table for proper cleanup
        let old_page_table = process.page_table.take();

        log::info!(
            "exec_process: Preserving thread ID {} for process {}",
            thread_id,
            pid.as_u64()
        );

        // Load the new ELF program properly
        log::info!(
            "exec_process: Loading new ELF program ({} bytes)",
            elf_data.len()
        );

        // Create a new page table for the new program
        log::info!("exec_process: Creating new page table...");
        let mut new_page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|_| "Failed to create new page table for exec")?,
        );
        log::info!("exec_process: New page table created successfully");

        // Clear any user mappings that might have been copied from the current page table
        // This prevents conflicts when loading the new program
        new_page_table.clear_user_entries();

        // Unmap the old program's pages in common userspace ranges
        // This is necessary because entry 0 contains both kernel and user mappings
        // Typical userspace code location: USERSPACE_BASE + 1MB range
        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE),
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE + 0x100000)
        )
        {
            log::warn!("Failed to unmap old user code pages: {}", e);
        }

        // Also unmap any pages in the BSS/data area (just after code)
        if let Err(e) =
            new_page_table.unmap_user_pages(VirtAddr::new(0x10001000), VirtAddr::new(0x10010000))
        {
            log::warn!("Failed to unmap old user data pages: {}", e);
        }

        // CRITICAL: Unmap the stack region before mapping new stack pages
        // The stack is at 0x7FFF_FF00_0000 - 0x7FFF_FF01_0000 (PML4 entry 255)
        // This region may have inherited mappings from the parent process
        {
            const STACK_SIZE: usize = 64 * 1024; // 64KB stack
            const STACK_TOP: u64 = 0x7FFF_FF01_0000;
            let unmap_bottom = VirtAddr::new(STACK_TOP - STACK_SIZE as u64);
            let unmap_top = VirtAddr::new(STACK_TOP);
            if let Err(e) = new_page_table.unmap_user_pages(unmap_bottom, unmap_top) {
                log::warn!("Failed to unmap old stack pages: {}", e);
            }
        }

        log::info!("exec_process: Cleared potential user mappings from new page table");

        // Load the ELF binary into the new page table
        log::info!("exec_process: Loading ELF into new page table...");
        let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, new_page_table.as_mut())?;
        let new_entry_point = loaded_elf.entry_point.as_u64();
        log::info!(
            "exec_process: ELF loaded successfully, entry point: {:#x}",
            new_entry_point
        );

        // CRITICAL FIX: Allocate and map stack directly into the new process page table
        // We need to manually allocate stack pages and map them into the new page table,
        // not the current kernel page table
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        // Use address in valid USER_STACK_REGION (0x7FFF_FF00_0000 - 0x8000_0000_0000)
        const USER_STACK_TOP: u64 = 0x7FFF_FF01_0000;

        // Calculate stack range
        let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
        let stack_top = VirtAddr::new(USER_STACK_TOP);
        let _guard_page = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64 - 0x1000);

        // Map stack pages into the NEW process page table
        log::info!("exec_process: Mapping stack pages into new process page table");
        let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
        let end_page = Page::<Size4KiB>::containing_address(stack_top - 1u64);
        log::info!(
            "exec_process: Stack range: {:#x} - {:#x}",
            stack_bottom.as_u64(),
            stack_top.as_u64()
        );

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for exec stack")?;

            // Map into the NEW process page table with user-accessible permissions
            new_page_table.map_page(
                page,
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            )?;
        }

        // For now, we'll use a dummy stack object since we manually mapped the stack
        // In the future, we should refactor stack allocation to support mapping into specific page tables
        let new_stack = crate::memory::stack::allocate_stack_with_privilege(
            4096, // Dummy size - we already mapped the real stack
            crate::task::thread::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to create stack object")?;

        // Use our manually calculated stack top
        let new_stack_top = stack_top;

        log::info!(
            "exec_process: New entry point: {:#x}, new stack top: {:#x}",
            new_entry_point,
            new_stack_top
        );

        // Update the process with new program data
        // Update the process name to match the new program if provided
        // CRITICAL: The process name must match a binary on the test disk because
        // fork() uses the process name to reload the binary.
        if let Some(name) = program_name {
            process.name = String::from(name);
            log::info!("exec_process: Updated process name to '{}'", name);
        }
        process.entry_point = loaded_elf.entry_point;

        // Reset heap bounds for the new program - heap starts after ELF segments
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;

        // Reset signal handlers per POSIX: user-defined handlers become SIG_DFL,
        // SIG_IGN handlers are preserved
        process.signals.exec_reset();
        // Reset mmap state for the new address space
        process.mmap_hint = crate::memory::vma::MMAP_REGION_END;
        process.vmas.clear();
        log::debug!("exec_process: Reset signal/heap/mmap for process {}, heap_start={:#x}", pid.as_u64(), heap_base);

        // Replace the page table with the new one containing the loaded program
        process.page_table = Some(new_page_table);

        // Replace the stack
        process.stack = Some(Box::new(new_stack));

        // Update the main thread context for the new program
        if let Some(ref mut thread) = process.main_thread {
            // CRITICAL: Preserve the kernel stack - userspace threads need it for syscalls
            let preserved_kernel_stack_top = thread.kernel_stack_top;
            log::info!(
                "exec_process: Preserving kernel stack top: {:?}",
                preserved_kernel_stack_top
            );

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

            log::info!(
                "exec_process: Updated thread {} context for new program",
                thread_id
            );
        }

        log::info!(
            "exec_process: Successfully replaced process {} address space",
            pid.as_u64()
        );

        // CRITICAL OS-STANDARD: Handle page table switching based on process state
        if is_current_process {
            // This is the current process - we're in a syscall from it
            // In a real OS, exec() on the current process requires:
            // 1. The page table switch MUST be deferred until interrupt return
            // 2. We CANNOT switch page tables while executing kernel code
            // 3. The syscall return path will handle the actual switch

            // Schedule the page table switch for when we return to userspace
            // FIXED: CR3 switching now happens in the scheduler during context switch
            // When we return from this syscall and the next timer interrupt fires,
            // the scheduler will switch to the new page table if needed

            log::info!("exec_process: Current process exec - page table will be used on next context switch");

            // DO NOT flush TLB here - let the interrupt return path handle it
            // Flushing TLB while still using the old page table mappings is dangerous
            // The assembly code will handle the TLB flush after the page table switch
        } else if is_scheduled {
            // Process is scheduled but not current - it will pick up the new page table
            // when it's next scheduled to run. The context switch code will handle it.
            log::info!("exec_process: Process {} is scheduled - new page table will be used on next schedule", pid.as_u64());
            // The scheduler will use the process's page table during context switch
        } else {
            // Process is not scheduled - it will use the new page table when it runs
            log::info!(
                "exec_process: Process {} is not scheduled - new page table ready for when it runs",
                pid.as_u64()
            );
        }

        // Defer old page table cleanup: push onto the process's pending list.
        // We cannot free the old page table immediately because CR3 may still
        // reference it if a timer interrupt fires before the next context switch.
        // The old table will be cleaned up at the start of the next exec or
        // when the process exits.
        if let Some(old_pt) = old_page_table {
            log::info!("exec_process: Deferring old page table cleanup");
            if let Some(process) = self.processes.get_mut(&pid) {
                process.pending_old_page_tables.push(old_pt);
            }
        }

        // Add the process back to the ready queue if it's not already there
        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
            log::info!(
                "exec_process: Added process {} back to ready queue",
                pid.as_u64()
            );
        }

        // CRITICAL OS-STANDARD: exec() should NEVER return to the calling process
        // The process has been completely replaced. In a real implementation:
        // - If exec() succeeds, it never returns (jumps to new program)
        // - If exec() fails, it returns an error to the original program
        // For now, we return the entry point for testing, but this violates POSIX
        Ok(new_entry_point)
    }

    /// Replace a process's address space with a new program (exec) with argv support
    ///
    /// This is the extended version of exec_process that sets up argc/argv on the stack
    /// following the Linux/FreeBSD ABI convention:
    ///
    /// Stack layout at _start (from high to low addresses):
    /// - argv strings (null-terminated)
    /// - padding for 16-byte alignment
    /// - NULL (end of argv pointers)
    /// - argv[n-1] pointer
    /// - ...
    /// - argv[0] pointer
    /// - argc           <- RSP points here
    ///
    /// Parameters:
    /// - pid: Process ID to exec
    /// - elf_data: The ELF binary data
    /// - program_name: Optional name for the process
    /// - argv: Array of argument strings (argv[0] is typically the program name)
    ///
    /// Returns: (entry_point, stack_pointer) on success
    /// Note: Exec requires architecture-specific register manipulation
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    pub fn exec_process_with_argv(
        &mut self,
        pid: ProcessId,
        elf_data: &[u8],
        program_name: Option<&str>,
        argv: &[&[u8]],
    ) -> Result<(u64, u64), &'static str> {
        log::info!(
            "exec_process_with_argv: Replacing process {} with new program, argc={}",
            pid.as_u64(),
            argv.len()
        );

        // CRITICAL OS-STANDARD CHECK: Is this the current process?
        let is_current_process = self.current_pid == Some(pid);
        if is_current_process {
            log::info!("exec_process_with_argv: Executing on current process - special handling required");
        }

        // For now, assume non-current processes are not actively running
        let is_scheduled = false;

        // Get thread ID and take old page table before dropping the mutable borrow
        // We need to do this early so we can call setup_argv_on_stack later
        let (thread_id, old_page_table) = {
            let process = self.processes.get_mut(&pid).ok_or("Process not found")?;
            // Drain any pending old page tables from previous exec() calls.
            process.drain_old_page_tables();
            let main_thread = process
                .main_thread
                .as_ref()
                .ok_or("Process has no main thread")?;
            let thread_id = main_thread.id;
            let old_page_table = process.page_table.take();
            (thread_id, old_page_table)
        };

        log::info!(
            "exec_process_with_argv: Preserving thread ID {} for process {}",
            thread_id,
            pid.as_u64()
        );

        // Create a new page table for the new program
        log::info!("exec_process_with_argv: Creating new page table...");
        let mut new_page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|_| "Failed to create new page table for exec")?,
        );

        // Clear any user mappings that might have been copied
        new_page_table.clear_user_entries();

        // Unmap old pages
        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE),
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE + 0x100000)
        ) {
            log::warn!("Failed to unmap old user code pages: {}", e);
        }

        if let Err(e) = new_page_table.unmap_user_pages(VirtAddr::new(0x10001000), VirtAddr::new(0x10010000)) {
            log::warn!("Failed to unmap old user data pages: {}", e);
        }

        // Unmap the stack region
        {
            const STACK_SIZE: usize = 64 * 1024;
            const STACK_TOP: u64 = 0x7FFF_FF01_0000;
            let unmap_bottom = VirtAddr::new(STACK_TOP - STACK_SIZE as u64);
            let unmap_top = VirtAddr::new(STACK_TOP);
            if let Err(e) = new_page_table.unmap_user_pages(unmap_bottom, unmap_top) {
                log::warn!("Failed to unmap old stack pages: {}", e);
            }
        }

        // Load the ELF binary into the new page table
        log::info!("exec_process_with_argv: Loading ELF into new page table...");
        let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, new_page_table.as_mut())?;
        let new_entry_point = loaded_elf.entry_point.as_u64();
        log::info!(
            "exec_process_with_argv: ELF loaded successfully, entry point: {:#x}",
            new_entry_point
        );

        // Map stack pages into the NEW process page table
        const USER_STACK_SIZE: usize = 64 * 1024;
        const USER_STACK_TOP: u64 = 0x7FFF_FF01_0000;

        let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
        let stack_top = VirtAddr::new(USER_STACK_TOP);

        log::info!("exec_process_with_argv: Mapping stack pages into new process page table");
        let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
        let end_page = Page::<Size4KiB>::containing_address(stack_top - 1u64);

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for exec stack")?;

            new_page_table.map_page(
                page,
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            )?;
        }

        // Set up argc/argv on the stack following Linux ABI
        // We need to write to the new stack pages that we just mapped
        // Since the new page table is not active yet, we need to translate addresses
        // and write via the physical frames
        let initial_rsp = self.setup_argv_on_stack(&new_page_table, USER_STACK_TOP, argv)?;

        log::info!(
            "exec_process_with_argv: argc/argv set up on stack, RSP={:#x}",
            initial_rsp
        );

        // Create a dummy stack object since we manually mapped the stack
        let new_stack = crate::memory::stack::allocate_stack_with_privilege(
            4096,
            crate::task::thread::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to create stack object")?;

        // Re-borrow the process for the remaining updates
        let process = self.processes.get_mut(&pid).ok_or("Process not found during update")?;

        // Update the process with new program data
        if let Some(name) = program_name {
            process.name = String::from(name);
            log::info!("exec_process_with_argv: Updated process name to '{}'", name);
        }
        process.entry_point = loaded_elf.entry_point;

        // Reset heap bounds for the new program
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;

        // Reset signal handlers and mmap state per POSIX
        process.signals.exec_reset();
        process.mmap_hint = crate::memory::vma::MMAP_REGION_END;
        process.vmas.clear();

        // Replace the page table with the new one
        process.page_table = Some(new_page_table);
        process.stack = Some(Box::new(new_stack));

        // Update the main thread context for the new program
        if let Some(ref mut thread) = process.main_thread {
            let preserved_kernel_stack_top = thread.kernel_stack_top;

            // Reset the CPU context for the new program
            thread.context.rip = new_entry_point;
            thread.context.rsp = initial_rsp;  // Points to argc on stack
            thread.context.rflags = 0x202;
            thread.stack_top = stack_top;
            thread.stack_bottom = stack_bottom;
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

            thread.context.cs = 0x33;
            thread.context.ss = 0x2b;
            thread.state = crate::task::thread::ThreadState::Ready;

            log::info!(
                "exec_process_with_argv: Updated thread {} context for new program",
                thread_id
            );
        }

        // Handle page table switching
        if is_current_process {
            log::info!("exec_process_with_argv: Current process exec - page table will be used on next context switch");
        } else if is_scheduled {
            log::info!("exec_process_with_argv: Process {} is scheduled - new page table will be used on next schedule", pid.as_u64());
        } else {
            log::info!(
                "exec_process_with_argv: Process {} is not scheduled - new page table ready for when it runs",
                pid.as_u64()
            );
        }

        // Defer old page table cleanup (see exec_process for rationale)
        if let Some(old_pt) = old_page_table {
            log::info!("exec_process_with_argv: Deferring old page table cleanup");
            if let Some(process) = self.processes.get_mut(&pid) {
                process.pending_old_page_tables.push(old_pt);
            }
        }

        // Add the process back to the ready queue if it's not already there
        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
        }

        Ok((new_entry_point, initial_rsp))
    }

    /// Replace a process's address space with a new program (exec) with argv support (ARM64)
    ///
    /// Returns (entry_point, stack_pointer) on success.
    #[cfg(target_arch = "aarch64")]
    pub fn exec_process_with_argv(
        &mut self,
        pid: ProcessId,
        elf_data: &[u8],
        program_name: Option<&str>,
        argv: &[&[u8]],
    ) -> Result<(u64, u64), &'static str> {
        use crate::arch_impl::aarch64::constants::USER_STACK_REGION_START;
        use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB};

        log::info!(
            "exec_process_with_argv [ARM64]: Replacing process {} with new program, argc={}",
            pid.as_u64(),
            argv.len()
        );

        let is_current_process = self.current_pid == Some(pid);
        if is_current_process {
            log::info!(
                "exec_process_with_argv [ARM64]: Executing on current process - special handling required"
            );
        }

        let is_scheduled = false;

        let (thread_id, old_page_table) = {
            let process = self.processes.get_mut(&pid).ok_or("Process not found")?;
            // Drain any pending old page tables from previous exec() calls.
            process.drain_old_page_tables();
            let main_thread = process
                .main_thread
                .as_ref()
                .ok_or("Process has no main thread")?;
            let thread_id = main_thread.id;
            let old_page_table = process.page_table.take();
            (thread_id, old_page_table)
        };

        log::info!(
            "exec_process_with_argv [ARM64]: Preserving thread ID {} for process {}",
            thread_id,
            pid.as_u64()
        );

        let mut new_page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|_| "Failed to create new page table for exec")?,
        );

        new_page_table.clear_user_entries();

        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE),
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE + 0x100000),
        ) {
            log::warn!("ARM64: Failed to unmap old user code pages: {}", e);
        }

        if let Err(e) =
            new_page_table.unmap_user_pages(VirtAddr::new(0x10001000), VirtAddr::new(0x10010000))
        {
            log::warn!("ARM64: Failed to unmap old user data pages: {}", e);
        }

        let user_stack_top = USER_STACK_REGION_START;

        {
            const STACK_SIZE: usize = 64 * 1024;
            let unmap_bottom = VirtAddr::new(user_stack_top - STACK_SIZE as u64);
            let unmap_top = VirtAddr::new(user_stack_top);
            if let Err(e) = new_page_table.unmap_user_pages(unmap_bottom, unmap_top) {
                log::warn!("ARM64: Failed to unmap old stack pages: {}", e);
            }
        }

        log::info!("exec_process_with_argv [ARM64]: Loading ELF into new page table...");
        let loaded_elf =
            crate::arch_impl::aarch64::elf::load_elf_into_page_table(elf_data, new_page_table.as_mut())?;
        let new_entry_point = loaded_elf.entry_point;
        log::info!(
            "exec_process_with_argv [ARM64]: ELF loaded successfully, entry point: {:#x}",
            new_entry_point
        );

        const USER_STACK_SIZE: usize = 64 * 1024;

        let stack_bottom = VirtAddr::new(user_stack_top - USER_STACK_SIZE as u64);
        let stack_top = VirtAddr::new(user_stack_top);

        log::info!("exec_process_with_argv [ARM64]: Mapping stack pages into new process page table");
        let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(stack_top.as_u64() - 1));

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for exec stack")?;

            new_page_table.map_page(
                page,
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            )?;
        }

        let initial_rsp = self.setup_argv_on_stack(&new_page_table, user_stack_top, argv)?;

        log::info!(
            "exec_process_with_argv [ARM64]: argc/argv set up on stack, SP_EL0={:#x}",
            initial_rsp
        );

        let new_stack = crate::memory::stack::allocate_stack_with_privilege(
            4096,
            crate::memory::arch_stub::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to create stack object")?;

        let process = self
            .processes
            .get_mut(&pid)
            .ok_or("Process not found during update")?;

        if let Some(name) = program_name {
            process.name = String::from(name);
            log::info!(
                "exec_process_with_argv [ARM64]: Updated process name to '{}'",
                name
            );
        }
        process.entry_point = VirtAddr::new(new_entry_point);

        // Reset heap bounds for the new program
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;

        process.signals.exec_reset();
        process.mmap_hint = crate::memory::vma::MMAP_REGION_END;
        process.vmas.clear();

        process.page_table = Some(new_page_table);
        process.stack = Some(Box::new(new_stack));

        if let Some(ref mut thread) = process.main_thread {
            let preserved_kernel_stack_top = thread.kernel_stack_top;

            let aligned_stack = initial_rsp & !0xF;
            thread.context.elr_el1 = new_entry_point;
            thread.context.sp_el0 = aligned_stack;
            thread.context.spsr_el1 = 0x0;

            thread.context.x0 = 0;
            thread.context.x19 = 0;
            thread.context.x20 = 0;
            thread.context.x21 = 0;
            thread.context.x22 = 0;
            thread.context.x23 = 0;
            thread.context.x24 = 0;
            thread.context.x25 = 0;
            thread.context.x26 = 0;
            thread.context.x27 = 0;
            thread.context.x28 = 0;
            thread.context.x29 = 0;
            thread.context.x30 = 0;

            thread.stack_top = stack_top;
            thread.stack_bottom = stack_bottom;
            thread.kernel_stack_top = preserved_kernel_stack_top;
            thread.state = crate::task::thread::ThreadState::Ready;

            log::info!(
                "exec_process_with_argv [ARM64]: Updated thread {} context for new program",
                thread_id
            );

            // CRITICAL: Sync updated context to the scheduler's copy of this thread.
            // The process manager and scheduler maintain SEPARATE Thread objects (cloned
            // at process creation). Without this sync, the scheduler would restore stale
            // context (e.g., elr_el1=0) on the next context switch, causing ELR=0x0 crashes.
            let ctx = thread.context.clone();
            let st = thread.stack_top;
            let sb = thread.stack_bottom;
            let kst = thread.kernel_stack_top;
            crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                sched_thread.context = ctx;
                sched_thread.stack_top = st;
                sched_thread.stack_bottom = sb;
                sched_thread.kernel_stack_top = kst;
                sched_thread.state = crate::task::thread::ThreadState::Ready;
            });
        }

        if is_current_process {
            log::info!(
                "exec_process_with_argv [ARM64]: Current process exec - page table will be used on next context switch"
            );
        } else if is_scheduled {
            log::info!(
                "exec_process_with_argv [ARM64]: Process {} is scheduled - new page table will be used on next schedule",
                pid.as_u64()
            );
        } else {
            log::info!(
                "exec_process_with_argv [ARM64]: Process {} is not scheduled - new page table ready for when it runs",
                pid.as_u64()
            );
        }

        // Defer old page table cleanup (see exec_process for rationale)
        if let Some(old_pt) = old_page_table {
            log::info!("exec_process_with_argv [ARM64]: Deferring old page table cleanup");
            if let Some(process) = self.processes.get_mut(&pid) {
                process.pending_old_page_tables.push(old_pt);
            }
        }

        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
        }

        Ok((new_entry_point, initial_rsp))
    }

    /// Replace a process's address space with a new program (exec) for ARM64
    ///
    /// This implements the exec() family of system calls on ARM64. Unlike fork(), which creates
    /// a new process, exec() replaces the current process's address space with a new
    /// program while keeping the same PID.
    ///
    /// The `program_name` parameter is optional - if provided, it updates the process name
    /// to match the new program.
    ///
    /// ARM64-specific details:
    /// - Uses TTBR0_EL1 for userspace page tables (kernel is always in TTBR1)
    /// - ELR_EL1 holds the entry point (return PC)
    /// - SP_EL0 holds the user stack pointer
    /// - SPSR_EL1 = 0x0 for EL0t mode with interrupts enabled
    /// - X0-X30 are cleared for security
    #[cfg(target_arch = "aarch64")]
    pub fn exec_process(
        &mut self,
        pid: ProcessId,
        elf_data: &[u8],
        program_name: Option<&str>,
    ) -> Result<u64, &'static str> {
        use crate::arch_impl::aarch64::constants::USER_STACK_REGION_START;
        use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB};

        // Lock-free trace: exec entry
        crate::tracing::providers::process::trace_exec_entry(pid.as_u64() as u32);

        log::info!(
            "exec_process [ARM64]: Replacing process {} with new program",
            pid.as_u64()
        );

        // CRITICAL OS-STANDARD CHECK: Is this the current process?
        let is_current_process = self.current_pid == Some(pid);
        if is_current_process {
            log::info!("exec_process [ARM64]: Executing on current process - special handling required");
        }

        // Get the existing process
        let process = self.processes.get_mut(&pid).ok_or("Process not found")?;

        // Drain any pending old page tables from previous exec() calls.
        process.drain_old_page_tables();

        // For now, assume non-current processes are not actively running
        let is_scheduled = false;

        // Get the main thread (we need to preserve its ID)
        let main_thread = process
            .main_thread
            .as_ref()
            .ok_or("Process has no main thread")?;
        let thread_id = main_thread.id;

        // Store old page table for proper cleanup
        let old_page_table = process.page_table.take();

        log::info!(
            "exec_process [ARM64]: Preserving thread ID {} for process {}",
            thread_id,
            pid.as_u64()
        );

        // Create a new page table for the new program
        log::info!("exec_process [ARM64]: Creating new page table...");
        let mut new_page_table = Box::new(
            crate::memory::process_memory::ProcessPageTable::new()
                .map_err(|_| "Failed to create new page table for exec")?,
        );
        log::info!("exec_process [ARM64]: New page table created successfully");

        // Clear any user mappings that might have been copied from the current page table
        new_page_table.clear_user_entries();

        // Unmap the old program's pages in common userspace ranges
        if let Err(e) = new_page_table.unmap_user_pages(
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE),
            VirtAddr::new(crate::memory::layout::USERSPACE_BASE + 0x100000),
        ) {
            log::warn!("ARM64: Failed to unmap old user code pages: {}", e);
        }

        // Also unmap any pages in the BSS/data area (just after code)
        if let Err(e) =
            new_page_table.unmap_user_pages(VirtAddr::new(0x10001000), VirtAddr::new(0x10010000))
        {
            log::warn!("ARM64: Failed to unmap old user data pages: {}", e);
        }

        // Unmap the stack region before mapping new stack pages
        let user_stack_top = USER_STACK_REGION_START;

        {
            const STACK_SIZE: usize = 64 * 1024; // 64KB stack
            let unmap_bottom = VirtAddr::new(user_stack_top - STACK_SIZE as u64);
            let unmap_top = VirtAddr::new(user_stack_top);
            if let Err(e) = new_page_table.unmap_user_pages(unmap_bottom, unmap_top) {
                log::warn!("ARM64: Failed to unmap old stack pages: {}", e);
            }
        }

        log::info!("exec_process [ARM64]: Cleared potential user mappings from new page table");

        // Load the ELF binary into the new page table using ARM64-specific loader
        log::info!("exec_process [ARM64]: Loading ELF into new page table...");
        let loaded_elf =
            crate::arch_impl::aarch64::elf::load_elf_into_page_table(elf_data, new_page_table.as_mut())?;
        let new_entry_point = loaded_elf.entry_point;
        log::info!(
            "exec_process [ARM64]: ELF loaded successfully, entry point: {:#x}",
            new_entry_point
        );

        // Allocate and map stack directly into the new process page table
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack

        // Calculate stack range
        let stack_bottom = VirtAddr::new(user_stack_top - USER_STACK_SIZE as u64);
        let stack_top = VirtAddr::new(user_stack_top);

        // Map stack pages into the NEW process page table
        log::info!("exec_process [ARM64]: Mapping stack pages into new process page table");
        let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(stack_top.as_u64() - 1));
        log::info!(
            "exec_process [ARM64]: Stack range: {:#x} - {:#x}",
            stack_bottom.as_u64(),
            stack_top.as_u64()
        );

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("Failed to allocate frame for exec stack")?;

            // Map into the NEW process page table with user-accessible permissions
            new_page_table.map_page(
                page,
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            )?;
        }

        // For now, we'll use a dummy stack object since we manually mapped the stack
        let new_stack = crate::memory::stack::allocate_stack_with_privilege(
            4096, // Dummy size - we already mapped the real stack
            crate::memory::arch_stub::ThreadPrivilege::User,
        )
        .map_err(|_| "Failed to create stack object")?;

        log::info!(
            "exec_process [ARM64]: New entry point: {:#x}, new stack top: {:#x}",
            new_entry_point,
            stack_top.as_u64()
        );

        // Update the process with new program data
        if let Some(name) = program_name {
            process.name = String::from(name);
            log::info!("exec_process [ARM64]: Updated process name to '{}'", name);
        }
        process.entry_point = VirtAddr::new(new_entry_point);

        // Reset heap bounds for the new program
        let heap_base = loaded_elf.segments_end;
        process.heap_start = heap_base;
        process.heap_end = heap_base;

        // Reset signal handlers and mmap state per POSIX
        process.signals.exec_reset();
        process.mmap_hint = crate::memory::vma::MMAP_REGION_END;
        process.vmas.clear();
        log::debug!(
            "exec_process [ARM64]: Reset signal/heap/mmap for process {}, heap_start={:#x}",
            pid.as_u64(),
            heap_base
        );

        // Replace the page table with the new one containing the loaded program
        process.page_table = Some(new_page_table);

        // Replace the stack
        process.stack = Some(Box::new(new_stack));

        // Update the main thread context for the new program (ARM64-specific)
        if let Some(ref mut thread) = process.main_thread {
            // CRITICAL: Preserve the kernel stack - userspace threads need it for exceptions
            let preserved_kernel_stack_top = thread.kernel_stack_top;
            log::info!(
                "exec_process [ARM64]: Preserving kernel stack top: {:?}",
                preserved_kernel_stack_top
            );

            // Reset the CPU context for the new program (ARM64-specific registers)
            // SP must be 16-byte aligned on ARM64
            let aligned_stack = stack_top.as_u64() & !0xF;

            // Set ARM64-specific context fields
            thread.context.elr_el1 = new_entry_point; // Entry point (PC on return)
            thread.context.sp_el0 = aligned_stack; // User stack pointer
            thread.context.spsr_el1 = 0x0; // EL0t mode with interrupts enabled

            // Clear all general-purpose registers for security
            thread.context.x0 = 0;
            thread.context.x19 = 0;
            thread.context.x20 = 0;
            thread.context.x21 = 0;
            thread.context.x22 = 0;
            thread.context.x23 = 0;
            thread.context.x24 = 0;
            thread.context.x25 = 0;
            thread.context.x26 = 0;
            thread.context.x27 = 0;
            thread.context.x28 = 0;
            thread.context.x29 = 0; // Frame pointer
            thread.context.x30 = 0; // Link register

            thread.stack_top = stack_top;
            thread.stack_bottom = stack_bottom;

            // Restore the preserved kernel stack
            thread.kernel_stack_top = preserved_kernel_stack_top;

            // Mark the thread as ready to run the new program
            thread.state = crate::task::thread::ThreadState::Ready;

            log::info!(
                "exec_process [ARM64]: Updated thread {} context for new program",
                thread_id
            );

            // CRITICAL: Sync updated context to the scheduler's copy of this thread.
            // See exec_process_with_argv for detailed explanation of the dual-storage issue.
            let ctx = thread.context.clone();
            let st = thread.stack_top;
            let sb = thread.stack_bottom;
            let kst = thread.kernel_stack_top;
            crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                sched_thread.context = ctx;
                sched_thread.stack_top = st;
                sched_thread.stack_bottom = sb;
                sched_thread.kernel_stack_top = kst;
                sched_thread.state = crate::task::thread::ThreadState::Ready;
            });
        }

        log::info!(
            "exec_process [ARM64]: Successfully replaced process {} address space",
            pid.as_u64()
        );

        // Handle page table switching based on process state
        if is_current_process {
            log::info!("exec_process [ARM64]: Current process exec - page table will be used on next context switch");
        } else if is_scheduled {
            log::info!(
                "exec_process [ARM64]: Process {} is scheduled - new page table will be used on next schedule",
                pid.as_u64()
            );
        } else {
            log::info!(
                "exec_process [ARM64]: Process {} is not scheduled - new page table ready for when it runs",
                pid.as_u64()
            );
        }

        // Defer old page table cleanup (see exec_process x86_64 for rationale)
        if let Some(old_pt) = old_page_table {
            log::info!("exec_process [ARM64]: Deferring old page table cleanup");
            if let Some(process) = self.processes.get_mut(&pid) {
                process.pending_old_page_tables.push(old_pt);
            }
        }

        // Add the process back to the ready queue if it's not already there
        if !self.ready_queue.contains(&pid) {
            self.ready_queue.push(pid);
            log::info!(
                "exec_process [ARM64]: Added process {} back to ready queue",
                pid.as_u64()
            );
        }

        // Lock-free trace: exec exit
        crate::tracing::providers::process::trace_exec_exit(pid.as_u64() as u32);

        Ok(new_entry_point)
    }

    /// Set up argc/argv on the stack for a new process
    ///
    /// This function writes the argc/argv structure to the stack following the
    /// Linux x86_64 ABI convention. The stack layout at _start is:
    ///
    /// High addresses:
    ///   argv strings (null-terminated, packed)
    ///   padding for 16-byte alignment
    ///   NULL (end of argv)
    ///   argv[n-1] pointer
    ///   ...
    ///   argv[0] pointer
    ///   argc               <- RSP points here
    /// Low addresses:
    ///
    /// Parameters:
    /// - page_table: The process's page table (for translating virtual to physical addresses)
    /// - stack_top: The top of the stack (highest address)
    /// - argv: Array of argument strings (each must be null-terminated)
    ///
    /// Returns: The initial RSP value (pointing to argc)
    #[allow(dead_code)]
    fn setup_argv_on_stack(
        &self,
        page_table: &crate::memory::process_memory::ProcessPageTable,
        stack_top: u64,
        argv: &[&[u8]],
    ) -> Result<u64, &'static str> {
        let argc = argv.len();

        // We need to access the stack memory directly via physical addresses
        // since the new page table isn't active yet

        // Calculate total space needed for strings
        let mut total_string_space: usize = 0;
        for arg in argv.iter() {
            // Each string + null terminator (if not already null-terminated)
            let len = arg.len();
            if len > 0 && arg[len - 1] == 0 {
                total_string_space += len;
            } else {
                total_string_space += len + 1;
            }
        }

        // Start placing strings at the top of the stack and work down
        let mut string_ptr = stack_top;

        // Reserve space for strings
        string_ptr -= total_string_space as u64;

        // Align down to 8 bytes for string area
        string_ptr = string_ptr & !7;

        // We'll collect the string addresses as we write them
        let mut string_addresses: Vec<u64> = Vec::with_capacity(argc);

        // Write strings from the reserved area upward
        let mut current_string_addr = string_ptr;

        for arg in argv.iter() {
            string_addresses.push(current_string_addr);

            // Write the string bytes
            for byte in arg.iter() {
                self.write_byte_to_stack(page_table, current_string_addr, *byte)?;
                current_string_addr += 1;
            }

            // Add null terminator if not present
            let len = arg.len();
            if len == 0 || arg[len - 1] != 0 {
                self.write_byte_to_stack(page_table, current_string_addr, 0)?;
                current_string_addr += 1;
            }
        }

        // Now place the pointer array and argc below the strings
        // Layout (from high to low):
        //   strings (already placed)
        //   NULL (8 bytes)
        //   argv[n-1] pointer (8 bytes)
        //   ...
        //   argv[0] pointer (8 bytes)
        //   argc (8 bytes)

        // Calculate space needed for pointers + argc
        let pointers_space = (argc + 1) * 8 + 8; // argc pointers + NULL + argc value

        // Start of pointer area (below strings)
        let mut ptr_area = string_ptr - pointers_space as u64;

        // Align to 16 bytes (required by x86_64 ABI)
        ptr_area = ptr_area & !15;

        // Write argc at the bottom
        let rsp = ptr_area;
        self.write_u64_to_stack(page_table, rsp, argc as u64)?;

        // Write argv pointers
        let argv_start = rsp + 8;
        for (i, addr) in string_addresses.iter().enumerate() {
            self.write_u64_to_stack(page_table, argv_start + (i * 8) as u64, *addr)?;
        }

        // Write NULL terminator for argv array
        self.write_u64_to_stack(page_table, argv_start + (argc * 8) as u64, 0)?;

        log::debug!(
            "setup_argv_on_stack: argc={}, RSP={:#x}, argv[0] at {:#x}",
            argc,
            rsp,
            if !string_addresses.is_empty() { string_addresses[0] } else { 0 }
        );

        Ok(rsp)
    }

    /// Write a single byte to the stack via physical address translation
    #[allow(dead_code)]
    fn write_byte_to_stack(
        &self,
        page_table: &crate::memory::process_memory::ProcessPageTable,
        virt_addr: u64,
        value: u8,
    ) -> Result<(), &'static str> {
        // Translate virtual address to physical
        // NOTE: translate_page uses translate_addr which returns the FULL physical
        // address including the page offset - do NOT add page_offset again!
        let phys_addr = page_table.translate_page(VirtAddr::new(virt_addr))
            .ok_or("Failed to translate stack address")?;

        // Write via direct physical memory mapping
        // The kernel has a direct mapping of all physical memory
        let phys_offset = crate::memory::physical_memory_offset();
        let kernel_virt = phys_offset + phys_addr.as_u64();

        unsafe {
            core::ptr::write_volatile(kernel_virt.as_mut_ptr::<u8>(), value);
        }

        Ok(())
    }

    /// Write a u64 to the stack via physical address translation
    #[allow(dead_code)]
    fn write_u64_to_stack(
        &self,
        page_table: &crate::memory::process_memory::ProcessPageTable,
        virt_addr: u64,
        value: u64,
    ) -> Result<(), &'static str> {
        // Write as 8 individual bytes to handle potential page boundaries
        // (though in practice argv data shouldn't cross page boundaries)
        let bytes = value.to_le_bytes();
        for (i, byte) in bytes.iter().enumerate() {
            self.write_byte_to_stack(page_table, virt_addr + i as u64, *byte)?;
        }
        Ok(())
    }
}
