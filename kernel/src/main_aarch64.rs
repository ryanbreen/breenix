//! ARM64 kernel entry point and initialization.
//!
//! This file contains the AArch64-specific kernel entry point.
//! It's completely separate from the x86_64 boot path which uses
//! the rust-osdev bootloader.
//!
//! Boot sequence:
//! 1. _start (assembly) - Set up stack, zero BSS, jump to kernel_main
//! 2. kernel_main - Initialize serial, timer, GIC, print "Hello"
//! 3. Eventually: Set up MMU, exceptions, userspace

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

// On non-aarch64, this binary is a stub. All real code is gated.
#[cfg(target_arch = "aarch64")]
extern crate alloc;
#[cfg(target_arch = "aarch64")]
extern crate rlibc; // Provides memcpy, memset, etc.

#[cfg(target_arch = "aarch64")]
use core::panic::PanicInfo;

// Import the kernel library macros and modules
#[cfg(target_arch = "aarch64")]
#[macro_use]
extern crate kernel;

#[cfg(target_arch = "aarch64")]
#[cfg_attr(any(feature = "kthread_test_only", feature = "kthread_stress_test", feature = "workqueue_test_only"), allow(dead_code))]
fn run_userspace_from_ext2(path: &str) -> Result<core::convert::Infallible, &'static str> {
    use alloc::string::String;
    use core::arch::asm;
    use kernel::arch_impl::aarch64::context::return_to_userspace;

    // Raw serial character output - no locks, minimal code
    fn raw_char(c: u8) {
        let addr = kernel::platform_config::uart_virt() as *mut u32;
        unsafe { core::ptr::write_volatile(addr, c as u32); }
    }

    // Markers: A=entry, B=got fs, C=resolved, D=read inode, E=read content,
    // F=ELF ok, G=process created, H=info extracted, I=scheduler reg,
    // J=percpu set, K=pid set, L=ttbr0 set, M=jumping to userspace

    raw_char(b'A'); // Entry - about to call root_fs_read()
    raw_char(b'a'); // Calling root_fs_read() now
    let fs_guard = kernel::fs::ext2::root_fs_read();
    raw_char(b'b'); // root_fs_read() returned, checking if Some
    let fs = fs_guard.as_ref().ok_or("ext2 root filesystem not mounted")?;
    raw_char(b'B'); // Got fs

    let inode_num = fs.resolve_path(path).map_err(|_| "init_shell not found")?;
    raw_char(b'C'); // Path resolved

    let inode = fs.read_inode(inode_num).map_err(|_| "failed to read inode")?;
    raw_char(b'D'); // Inode read

    if inode.is_dir() {
        return Err("init_shell is a directory");
    }

    raw_char(b'd'); // About to read file content

    // Disable interrupts during large file read to prevent timer overhead
    unsafe { kernel::arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts(); }

    let elf_data = fs.read_file_content(&inode).map_err(|_| "failed to read init_shell")?;
    raw_char(b'E'); // File content read

    // CRITICAL: Release ext2 lock BEFORE creating process and jumping to userspace.
    // return_to_userspace() never returns, so fs_guard would never be dropped.
    // If we hold the lock, fork/exec in userspace will deadlock trying to acquire it.
    drop(fs_guard);

    // Re-enable interrupts
    raw_char(b'e'); // About to enable interrupts

    // Check timer status before enabling
    let timer_ctl: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) timer_ctl);
    }
    // Print timer status: bit 0 = enabled, bit 1 = masked, bit 2 = pending
    raw_char(if timer_ctl & 1 != 0 { b'E' } else { b'-' });  // Timer enabled?
    raw_char(if timer_ctl & 2 != 0 { b'M' } else { b'-' });  // Timer masked?
    raw_char(if timer_ctl & 4 != 0 { b'P' } else { b'-' });  // Timer pending?

    unsafe { kernel::arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts(); }
    raw_char(b'f'); // Interrupts enabled

    if elf_data.len() < 4 || &elf_data[0..4] != b"\x7fELF" {
        return Err("init_shell is not a valid ELF file");
    }
    raw_char(b'F'); // ELF verified

    let proc_name = path.rsplit('/').next().unwrap_or(path);

    // Set up argv with the program name as argv[0]
    // The path (e.g., "/bin/init_shell") becomes argv[0]
    let argv: [&[u8]; 1] = [path.as_bytes()];

    let pid = {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.create_process_with_argv(String::from(proc_name), &elf_data, &argv)?
        } else {
            return Err("process manager not initialized");
        }
    };
    raw_char(b'G'); // Process created

    // Advance test stage to ProcessContext - a user process now exists with an fd_table
    // This allows tests that need process context (like sys_socket) to run
    #[cfg(feature = "boot_tests")]
    {
        let failures = kernel::test_framework::advance_to_stage(
            kernel::test_framework::TestStage::ProcessContext
        );
        if failures > 0 {
            kernel::serial_println!("[boot_tests] {} ProcessContext test(s) failed", failures);
        }
    }

    let (entry_point, user_sp, ttbr0_phys, main_thread_id, main_thread_clone) = {
        let manager_guard = kernel::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some(process) = manager.get_process(pid) {
                let entry = process.entry_point.as_u64();
                let thread = process
                    .main_thread
                    .as_ref()
                    .ok_or("process has no main thread")?;
                // Get the SP from the thread's context (points to argc on the stack)
                // For ARM64 userspace threads, the initial SP is stored in sp_el0
                let sp = thread.context.sp_el0;
                let ttbr0 = process
                    .page_table
                    .as_ref()
                    .ok_or("process has no page table")?
                    .level_4_frame()
                    .start_address()
                    .as_u64();
                (entry, sp, ttbr0, thread.id, thread.clone())
            } else {
                return Err("process not found after creation");
            }
        } else {
            return Err("process manager not available");
        }
    };
    raw_char(b'H'); // Process info extracted

    // Register the userspace thread with the scheduler as the current running thread.
    kernel::task::scheduler::spawn_as_current(alloc::boxed::Box::new(main_thread_clone));
    raw_char(b'I'); // Scheduler registered

    // CRITICAL: Reset the idle thread's (thread 0) saved context to point to idle_loop_arm64.
    // Without this, timer interrupts during boot may have saved thread 0's ELR pointing to
    // somewhere in kernel_main. When we later switch back to thread 0, it would resume
    // kernel_main and potentially create multiple init_shell processes.
    // By resetting elr_el1 to idle_loop_arm64, we ensure thread 0 always goes to the idle loop.
    kernel::task::scheduler::with_thread_mut(0, |idle_thread| {
        // Get the address of the idle loop function
        let idle_loop_addr = kernel::arch_impl::aarch64::context_switch::idle_loop_arm64 as *const () as u64;
        idle_thread.context.elr_el1 = idle_loop_addr;
        // Also set SPSR for EL1h with interrupts enabled
        idle_thread.context.spsr_el1 = 0x5; // EL1h, DAIF clear
        serial_println!("[boot] Reset idle thread context to idle_loop_arm64 at {:#x}", idle_loop_addr);
    });

    // Set per-CPU pointers to the thread in the scheduler
    kernel::task::scheduler::with_thread_mut(main_thread_id, |thread| {
        let thread_ptr = thread as *mut kernel::task::thread::Thread;
        kernel::per_cpu_aarch64::set_current_thread(thread_ptr);
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            kernel::per_cpu_aarch64::set_kernel_stack_top(kernel_stack_top.as_u64());
            // CRITICAL: Also set user_rsp_scratch so the boot.S ERET path
            // restores SP to the correct kernel stack. return_to_userspace()
            // sets SP_EL1 from percpu.kernel_stack_top, but the boot.S ERET
            // path uses user_rsp_scratch (offset 40) to set SP before ERET.
            // Without this, the first timer IRQ return without a context switch
            // would set SP_EL1 to the stale boot stack from user_rsp_scratch,
            // causing subsequent exception frames to be pushed on the wrong stack.
            kernel::per_cpu_aarch64::set_user_rsp_scratch(kernel_stack_top.as_u64());
        }
    });
    raw_char(b'J'); // Per-CPU set

    // Mark the process as running.
    {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.set_current_pid(pid);
        }
    }
    raw_char(b'K'); // Current PID set

    unsafe {
        asm!("msr ttbr0_el1, {0}", "isb", in(reg) ttbr0_phys, options(nostack, preserves_flags));
    }
    raw_char(b'L'); // TTBR0 set

    raw_char(b'M'); // Jumping to userspace
    unsafe { return_to_userspace(entry_point, user_sp); }
}


#[cfg(target_arch = "aarch64")]
use kernel::serial;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::timer;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::timer_interrupt;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::cpu::Aarch64Cpu;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::gic::Gicv2;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::traits::{CpuOps, InterruptController};
#[cfg(target_arch = "aarch64")]
use kernel::graphics::arm64_fb;
#[cfg(target_arch = "aarch64")]
use kernel::graphics::particles;
#[cfg(target_arch = "aarch64")]
use kernel::graphics::primitives::{draw_vline, fill_rect, Color, Rect};
#[cfg(target_arch = "aarch64")]
use kernel::drivers::virtio::input_mmio;

/// Kernel entry point called from assembly boot code.
#[cfg(target_arch = "aarch64")]
///
/// At this point:
/// - We're running at EL1 (or need to drop from EL2)
/// - Stack is set up
/// - BSS is zeroed
/// - MMU is already enabled by boot.S (high-half kernel)
#[no_mangle]
#[cfg_attr(feature = "kthread_test_only", allow(unreachable_code))]
pub extern "C" fn kernel_main(hw_config_ptr: u64) -> ! {
    // If the UEFI loader passed a HardwareConfig, use it to configure platform
    // addresses before any hardware access. On QEMU (boot.S), x0 is 0.
    if hw_config_ptr != 0 {
        let config = unsafe { &*(hw_config_ptr as *const kernel::platform_config::HardwareConfig) };
        kernel::platform_config::init_from_parallels(config);
    }

    // Initialize physical memory offset (needed for MMIO access)
    kernel::memory::init_physical_memory_offset_aarch64();

    // Initialize serial output first so we can print
    serial::init_serial();

    // Initialize the /proc/kmsg log buffer early so ALL serial output is captured
    kernel::log_buffer::init();

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Kernel Starting");
    serial_println!("========================================");
    serial_println!();

    // Print CPU info
    let el = current_exception_level();
    serial_println!("[boot] Current exception level: EL{}", el);

    serial_println!("[boot] MMU already enabled (high-half kernel)");

    // Initialize memory management for ARM64
    // ARM64 QEMU virt machine: RAM starts at 0x40000000
    // Frame allocator range from platform_config (QEMU defaults or HardwareConfig)
    let fa_start = kernel::platform_config::frame_alloc_start();
    let fa_end = kernel::platform_config::frame_alloc_end();
    serial_println!("[boot] Initializing memory management ({:#x}-{:#x})...", fa_start, fa_end);
    kernel::memory::frame_allocator::init_aarch64(fa_start, fa_end);
    kernel::memory::init_aarch64_heap();
    kernel::memory::kernel_stack::init();
    serial_println!("[boot] Memory management ready");

    // Initialize BTRT (requires memory and serial)
    #[cfg(feature = "btrt")]
    {
        use kernel::test_framework::{btrt, catalog};
        btrt::init();
        btrt::pass(catalog::KERNEL_ENTRY);
        btrt::pass(catalog::AARCH64_UART_INIT);
        btrt::pass(catalog::AARCH64_MMU_INIT);
        btrt::pass(catalog::MEMORY_INIT);
        btrt::pass(catalog::HEAP_INIT);
        btrt::pass(catalog::FRAME_ALLOC_INIT);
    }

    // Initialize timer
    serial_println!("[boot] Initializing Generic Timer...");
    timer::calibrate();
    let freq = timer::frequency_hz();
    serial_println!("[boot] Timer frequency: {} Hz ({} MHz)", freq, freq / 1_000_000);

    // Initialize RTC for wall-clock time (PL031 on QEMU virt)
    serial_println!("[boot] Initializing PL031 RTC...");
    kernel::time::rtc::init();

    // Read current timestamp
    let ts = timer::rdtsc();
    serial_println!("[boot] Current timestamp: {}", ts);

    // Initialize GIC
    serial_println!("[boot] Initializing GICv2...");
    Gicv2::init();
    serial_println!("[boot] GIC initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::AARCH64_GIC_INIT);

    // Enable UART receive interrupt (IRQ 33 = SPI 1)
    serial_println!("[boot] Enabling UART interrupts...");

    // Enable IRQ 33 in GIC (PL011 UART)
    serial_println!("[boot] Enabling GIC IRQ 33 (UART0)...");
    Gicv2::enable_irq(33); // UART0 IRQ

    // Enable RX interrupts in PL011
    serial::enable_rx_interrupt();

    // Dump GIC state for UART IRQ to verify configuration
    kernel::arch_impl::aarch64::gic::dump_irq_state(33);

    serial_println!("[boot] UART interrupts enabled");

    // Enable interrupts
    serial_println!("[boot] Enabling interrupts...");
    unsafe { Aarch64Cpu::enable_interrupts(); }
    let irq_enabled = Aarch64Cpu::interrupts_enabled();
    serial_println!("[boot] Interrupts enabled: {}", irq_enabled);

    // Read display resolution from fw_cfg before driver init
    kernel::drivers::virtio::gpu_mmio::load_resolution_from_fw_cfg();

    // Initialize device drivers (VirtIO MMIO enumeration)
    serial_println!("[boot] Initializing device drivers...");
    let device_count = kernel::drivers::init();
    serial_println!("[boot] Found {} devices", device_count);
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::PCI_ENUMERATION);

    // Initialize network stack (after VirtIO network driver is ready)
    serial_println!("[boot] Initializing network stack...");
    kernel::net::init();
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::NETWORK_STACK_INIT);

    // Initialize filesystem layer (requires VirtIO block device)
    serial_println!("[boot] Initializing filesystem...");

    // Initialize ext2 root filesystem (if block device present)
    match kernel::fs::ext2::init_root_fs() {
        Ok(()) => {
            serial_println!("[boot] ext2 root filesystem mounted");
            #[cfg(feature = "btrt")]
            kernel::test_framework::btrt::pass(kernel::test_framework::catalog::EXT2_MOUNT);
        }
        Err(e) => {
            serial_println!("[boot] ext2 init: {} (continuing without root fs)", e);
            #[cfg(feature = "btrt")]
            kernel::test_framework::btrt::fail(
                kernel::test_framework::catalog::EXT2_MOUNT,
                kernel::test_framework::btrt::BtrtErrorCode::IoError,
                0,
            );
        }
    }

    // Initialize ext2 home filesystem (/home on separate disk)
    match kernel::fs::ext2::init_home_fs() {
        Ok(()) => serial_println!("[boot] ext2 home filesystem mounted at /home"),
        Err(e) => serial_println!("[boot] No home filesystem: {} (continuing)", e),
    }

    // Initialize devfs (/dev virtual filesystem)
    kernel::fs::devfs::init();
    serial_println!("[boot] devfs initialized at /dev");

    // Initialize devptsfs (/dev/pts pseudo-terminal slave filesystem)
    kernel::fs::devptsfs::init();
    serial_println!("[boot] devptsfs initialized at /dev/pts");

    // Detect CPU features (must be before procfs so /proc/cpuinfo has real data)
    kernel::arch_impl::aarch64::cpuinfo::init();
    serial_println!("[boot] CPU detected: {} {}",
        kernel::arch_impl::aarch64::cpuinfo::get()
            .map(|c| c.implementer_name())
            .unwrap_or("Unknown"),
        kernel::arch_impl::aarch64::cpuinfo::get()
            .map(|c| c.part_name())
            .unwrap_or("Unknown"));

    // Initialize procfs (/proc virtual filesystem)
    kernel::fs::procfs::init();
    serial_println!("[boot] procfs initialized at /proc");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::PROCFS_INIT);

    // Initialize TTY subsystem (console + PTY infrastructure)
    kernel::tty::init();
    serial_println!("[boot] TTY subsystem initialized");

    // Initialize graphics (if GPU is available)
    serial_println!("[boot] Initializing graphics...");
    if let Err(e) = init_graphics() {
        serial_println!("[boot] Graphics init failed: {} (continuing without graphics)", e);
    }

    // Initialize VirtIO keyboard
    serial_println!("[boot] Initializing VirtIO keyboard...");
    match input_mmio::init() {
        Ok(()) => serial_println!("[boot] VirtIO keyboard initialized"),
        Err(e) => serial_println!("[boot] VirtIO keyboard init failed: {}", e),
    }

    // Initialize per-CPU data (required before scheduler and interrupts)
    serial_println!("[boot] Initializing per-CPU data...");
    kernel::per_cpu_aarch64::init();
    serial_println!("[boot] Per-CPU data initialized");

    // Initialize process manager
    serial_println!("[boot] Initializing process manager...");
    kernel::process::init();
    serial_println!("[boot] Process manager initialized");

    // Initialize scheduler with an idle task
    serial_println!("[boot] Initializing scheduler...");
    init_scheduler();
    serial_println!("[boot] Scheduler initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::SCHEDULER_INIT);

    // Initialize workqueue subsystem (depends on kthread infrastructure)
    kernel::task::workqueue::init_workqueue();
    serial_println!("[boot] Workqueue subsystem initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::WORKQUEUE_INIT);

    // Initialize softirq subsystem (depends on kthread infrastructure)
    kernel::task::softirqd::init_softirq();
    serial_println!("[boot] Softirq subsystem initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::KTHREAD_SUBSYSTEM);

    // Spawn render thread for deferred framebuffer rendering
    // This MUST come after scheduler is initialized (needs kthread infrastructure).
    //
    // Boot graphics architecture (shared with x86_64):
    // - Both architectures use render_task::spawn_render_thread() for deferred rendering
    // - Both use SHELL_FRAMEBUFFER (arm64_fb.rs on ARM64, logger.rs on x86_64)
    // - Both use graphics::render_queue for lock-free echo from interrupt context
    // - BWM (userspace window manager) handles terminal rendering
    // - Boot test progress display (test_framework::display) renders to SHELL_FRAMEBUFFER
    // - Boot milestones are tracked via BTRT (test_framework::btrt) on both platforms
    match kernel::graphics::render_task::spawn_render_thread() {
        Ok(tid) => serial_println!("[boot] Render thread spawned (tid={})", tid),
        Err(e) => serial_println!("[boot] Failed to spawn render thread: {}", e),
    }

    // Initialize timer interrupt for preemptive scheduling
    // This MUST come after per-CPU data and scheduler are initialized
    serial_println!("[boot] Initializing timer interrupt...");
    timer_interrupt::init();
    serial_println!("[boot] Timer interrupt initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::AARCH64_TIMER_INIT);

    // Bring up secondary CPUs via PSCI CPU_ON
    serial_println!("[smp] Starting secondary CPUs...");
    let expected_cpus: u64 = 4;
    for cpu in 1..expected_cpus {
        kernel::arch_impl::aarch64::smp::release_cpu(cpu as usize);
    }
    // Wait for all CPUs to come online (with timeout)
    let start = timer::rdtsc();
    let timeout_ticks = timer::frequency_hz() / 10; // 100ms timeout
    while kernel::arch_impl::aarch64::smp::cpus_online() < expected_cpus {
        if timer::rdtsc() - start > timeout_ticks {
            break;
        }
        core::hint::spin_loop();
    }
    serial_println!(
        "[smp] {} CPUs online",
        kernel::arch_impl::aarch64::smp::cpus_online()
    );

    // Test kthread lifecycle BEFORE creating userspace processes
    // (must be done early so scheduler doesn't preempt to userspace)
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_lifecycle();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_join();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_exit_code();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_park_unpark();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_double_stop();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_should_stop_non_kthread();
    #[cfg(feature = "testing")]
    kernel::task::kthread_tests::test_kthread_stop_after_exit();
    // Skip workqueue test in kthread_stress_test mode - it passes in Boot Stages
    // which has the same code but different build configuration.
    #[cfg(all(feature = "testing", not(feature = "kthread_stress_test")))]
    kernel::task::workqueue_tests::test_workqueue();
    #[cfg(all(feature = "testing", not(feature = "kthread_stress_test")))]
    kernel::task::softirq_tests::test_softirq();

    // In kthread_test_only mode, exit immediately after kthread tests pass
    #[cfg(feature = "kthread_test_only")]
    {
        serial_println!("=== KTHREAD_TEST_ONLY: All kthread tests passed ===");
        serial_println!("KTHREAD_TEST_ONLY_COMPLETE");
        kernel::exit_qemu(kernel::QemuExitCode::Success);
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
        }
    }

    // In workqueue_test_only mode, exit immediately after workqueue test
    #[cfg(feature = "workqueue_test_only")]
    {
        serial_println!("=== WORKQUEUE_TEST_ONLY: All workqueue tests passed ===");
        serial_println!("WORKQUEUE_TEST_ONLY_COMPLETE");
        kernel::exit_qemu(kernel::QemuExitCode::Success);
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
        }
    }

    // In kthread_stress_test mode, run stress test and exit
    #[cfg(feature = "kthread_stress_test")]
    {
        kernel::task::kthread_tests::test_kthread_stress();
        serial_println!("=== KTHREAD_STRESS_TEST: All stress tests passed ===");
        serial_println!("KTHREAD_STRESS_TEST_COMPLETE");
        kernel::exit_qemu(kernel::QemuExitCode::Success);
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
        }
    }

    // Run parallel boot tests if enabled
    #[cfg(feature = "boot_tests")]
    {
        serial_println!("[boot] Running parallel boot tests...");
        #[cfg(feature = "btrt")]
        kernel::test_framework::btrt::pass(kernel::test_framework::catalog::BOOT_TESTS_START);
        let failures = kernel::test_framework::run_all_tests();
        if failures > 0 {
            serial_println!("[boot] {} test(s) failed!", failures);
        } else {
            serial_println!("[boot] All boot tests passed!");
        }
        #[cfg(feature = "btrt")]
        kernel::test_framework::btrt::pass(kernel::test_framework::catalog::BOOT_TESTS_COMPLETE);
    }

    // Finalize BTRT: in non-testing mode, finalize now (kernel milestones only).
    // In testing mode, auto-finalize happens via on_process_exit() when all
    // registered test processes have completed.
    #[cfg(all(feature = "btrt", not(feature = "testing")))]
    kernel::test_framework::btrt::finalize();

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Boot Complete!");
    serial_println!("========================================");
    serial_println!();
    serial_println!("Hello from ARM64!");
    serial_println!();

    // Raw char helper for debugging
    fn boot_raw_char(c: u8) {
        let addr = kernel::platform_config::uart_virt() as *mut u32;
        unsafe { core::ptr::write_volatile(addr, c as u32); }
    }

    // Spawn particle animation thread (if graphics is available and not running boot tests)
    // This MUST be done BEFORE userspace loading because run_userspace_from_ext2 never returns
    // DISABLED: Investigating EC=0x0 crash during fill_rect memcpy
    #[cfg(not(feature = "boot_tests"))]
    #[cfg(feature = "particle_animation")]  // Disabled by default - crashes with EC=0x0
    {
        let has_graphics = kernel::graphics::arm64_fb::SHELL_FRAMEBUFFER.get().is_some();
        if has_graphics {
            serial_println!("[graphics] Starting particle animation...");
            match kernel::task::spawn::spawn_thread("particles", particles::animation_thread_entry) {
                Ok(tid) => serial_println!("[graphics] Particle animation started (tid={})", tid),
                Err(e) => serial_println!("[graphics] Failed to start animation: {}", e),
            }
        }
    }

    // In testing mode, load test binaries from ext2 and let the scheduler
    // dispatch them. Do NOT call run_userspace_from_ext2() - its manual
    // spawn_as_current() + return_to_userspace() bypasses the scheduler and
    // conflicts with the 60+ test processes already in the ready queue.
    #[cfg(feature = "testing")]
    if device_count > 0 {
        serial_println!("[test] Loading test binaries from ext2...");
        load_test_binaries_from_ext2();
        serial_println!("[test] Test processes loaded - will run via timer interrupts");
        serial_println!("[test] Entering scheduler idle loop");
        // Print shell prompt to serial before enabling interrupts.
        // Once interrupts are enabled, the scheduler takes over and the BSP
        // (idle thread 0) enters idle_loop_arm64 - it won't return here.
        // The prompt signals to the test harness that boot is complete.
        serial_print!("breenix> ");
        // Enable interrupts - scheduler dispatches test processes via timer.
        unsafe { kernel::arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts(); }
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
        }
    }

    boot_raw_char(b'1'); // Before if statement

    // Try to load and run userspace init_shell from ext2 or test disk
    if device_count > 0 {
        boot_raw_char(b'2'); // Inside if
        serial_println!("[boot] Loading userspace init from ext2...");
        boot_raw_char(b'3'); // After serial_println
        match run_userspace_from_ext2("/sbin/init") {
            Err(e) => {
                serial_println!("[boot] Failed to load init from ext2: {}", e);
                serial_println!("[boot] Loading userspace init_shell from test disk...");
                match kernel::boot::test_disk::run_userspace_from_disk("init_shell") {
                    Err(e) => {
                        serial_println!("[boot] Failed to load init_shell: {}", e);
                        serial_println!("[boot] Falling back to kernel shell...");
                    }
                    // run_userspace_from_disk returns Result<Infallible, _>, so Ok is unreachable
                    Ok(never) => match never {},
                }
            }
            Ok(never) => match never {},
        }
    }

    // No userspace init loaded — idle the kernel.
    // With the kernel shell removed, there's nothing to do here except
    // keep the kernel alive so interrupt-driven subsystems (timer, scheduler)
    // continue running.
    serial_println!("[interactive] No userspace init — idling");
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Load test binaries from ext2 filesystem and create userspace processes.
///
/// Each test binary is loaded from /bin/<name>.elf, parsed as ELF, and scheduled
/// via create_user_process(). The scheduler will run them alongside init_shell.
#[cfg(target_arch = "aarch64")]
#[cfg(feature = "testing")]
#[cfg_attr(any(feature = "kthread_test_only", feature = "kthread_stress_test", feature = "workqueue_test_only"), allow(dead_code))]
fn load_test_binaries_from_ext2() {
    use alloc::format;
    use alloc::string::String;

    // CRITICAL: Disable interrupts during the entire loading loop.
    // With interrupts enabled, each create_user_process() adds a thread to the
    // scheduler's ready queue. Timer interrupts (200Hz) then preempt this loading
    // thread to run the newly created test processes. By binary #30, the loading
    // thread competes with 30+ threads for CPU time and loading takes >90 seconds.
    // With interrupts disabled, VirtIO block I/O still works (polling mode) and
    // all binaries load in under a second.
    unsafe { kernel::arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts(); }

    // Use the canonical shared test binary list (see boot::test_list)
    let test_binaries = kernel::boot::test_list::TEST_BINARIES;

    let mut loaded = 0;
    let mut failed = 0;

    // Search paths for test binaries - try each in order
    let search_dirs = ["/bin", "/usr/local/cbin", "/usr/local/test/bin", "/sbin"];

    for name in test_binaries {
        // Load ELF from ext2 - acquire and release lock for each binary
        let elf_data = {
            let fs_guard = kernel::fs::ext2::root_fs_read();
            let fs = match fs_guard.as_ref() {
                Some(fs) => fs,
                None => {
                    serial_println!("[test] ext2 not mounted, cannot load {}", name);
                    return;
                }
            };

            // Try each search directory until we find the binary
            let mut found_inode = None;
            for dir in &search_dirs {
                let path = format!("{}/{}", dir, name);
                if let Ok(num) = fs.resolve_path(&path) {
                    found_inode = Some(num);
                    break;
                }
            }

            let inode_num = match found_inode {
                Some(num) => num,
                None => {
                    // Binary not present in any search path - skip silently
                    continue;
                }
            };

            let inode = match fs.read_inode(inode_num) {
                Ok(inode) => inode,
                Err(e) => {
                    serial_println!("[test] Failed to read inode for {}: {}", name, e);
                    failed += 1;
                    continue;
                }
            };

            match fs.read_file_content(&inode) {
                Ok(data) => data,
                Err(e) => {
                    serial_println!("[test] Failed to read {}: {}", path, e);
                    failed += 1;
                    continue;
                }
            }
            // fs_guard dropped here, releasing ext2 lock
        };

        // Validate ELF magic
        if elf_data.len() < 4 || &elf_data[0..4] != b"\x7fELF" {
            serial_println!("[test] {} is not a valid ELF file", name);
            failed += 1;
            continue;
        }

        // Create userspace process (adds to scheduler ready queue)
        match kernel::process::creation::create_user_process(String::from(*name), &elf_data) {
            Ok(pid) => {
                serial_println!("[test] Loaded {} (PID {})", name, pid.as_u64());
                #[cfg(feature = "btrt")]
                if let Some(test_id) = kernel::test_framework::catalog::utest_name_to_id(name) {
                    kernel::test_framework::btrt::register_pid(pid.as_u64(), test_id);
                }
                loaded += 1;
            }
            Err(e) => {
                serial_println!("[test] Failed to create process {}: {}", name, e);
                #[cfg(feature = "btrt")]
                if let Some(test_id) = kernel::test_framework::catalog::utest_name_to_id(name) {
                    kernel::test_framework::btrt::fail(test_id, kernel::test_framework::btrt::BtrtErrorCode::NoExec, 0);
                }
                failed += 1;
            }
        }
    }

    // NOTE: Interrupts remain DISABLED here. The caller is responsible for
    // printing status messages and re-enabling interrupts before entering
    // the idle loop. If we re-enable here, the scheduler immediately preempts
    // the boot thread to run test processes, and subsequent serial_println!
    // calls in the caller never execute.

    serial_println!("[test] Loaded {}/{} test binaries ({} failed, {} not found)",
        loaded, test_binaries.len(), failed,
        test_binaries.len() - loaded - failed);
}

/// Initialize the scheduler with an idle thread (ARM64)
#[cfg(target_arch = "aarch64")]
fn init_scheduler() {
    use alloc::boxed::Box;
    use alloc::string::String;
    use kernel::task::thread::{Thread, ThreadState, ThreadPrivilege};
    use kernel::task::scheduler;
    use kernel::per_cpu_aarch64;
    use kernel::memory::arch_stub::VirtAddr;

    // CPU 0 boot stack top address — must match boot.S layout:
    // HHDM_BASE + STACK_REGION_BASE + (cpu_id + 1) * STACK_SIZE
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    const STACK_REGION_BASE: u64 = 0x4100_0000;
    const STACK_SIZE: u64 = 0x20_0000; // 2MB per CPU
    let boot_stack_top = VirtAddr::new(HHDM_BASE + STACK_REGION_BASE + STACK_SIZE);
    let boot_stack_bottom = VirtAddr::new(HHDM_BASE + STACK_REGION_BASE);
    let dummy_tls = VirtAddr::zero();

    // Create the idle task (thread ID 0)
    let mut idle_task = Box::new(Thread::new(
        String::from("swapper/0"),  // Linux convention: swapper/0 is the idle task
        idle_thread_fn,
        boot_stack_top,
        boot_stack_bottom,
        dummy_tls,
        ThreadPrivilege::Kernel,
    ));

    // CRITICAL: Set kernel_stack_top to CPU 0's boot stack. Without this,
    // setup_idle_return_arm64 falls back to Aarch64PerCpu::kernel_stack_top()
    // which retains the LAST dispatched thread's kernel stack. The idle thread
    // then runs on that thread's stack, and timer IRQs push exception frames
    // that overwrite the other thread's SVC frame → ELR=0 crash.
    idle_task.kernel_stack_top = Some(boot_stack_top);

    // Mark as running with ID 0, and has_started=true since boot code is already executing
    idle_task.state = ThreadState::Running;
    idle_task.id = 0;
    idle_task.has_started = true;  // CRITICAL: Boot thread is already running, not waiting for first entry

    // Set up per-CPU current thread pointer and kernel stack
    let idle_task_ptr = &*idle_task as *const _ as *mut Thread;
    per_cpu_aarch64::set_current_thread(idle_task_ptr);
    per_cpu_aarch64::set_kernel_stack_top(boot_stack_top.as_u64());

    // Initialize scheduler with the idle task
    scheduler::init_with_current(idle_task);
}

/// Idle thread function - waits for interrupts when no work to do
#[cfg(target_arch = "aarch64")]
fn idle_thread_fn() {
    loop {
        // WFI saves power by halting until an interrupt arrives
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Test syscalls using SVC instruction from kernel mode.
/// This tests the basic exception handling and syscall dispatch.
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)] // Test function for manual debugging
fn test_syscalls() {
    // Test write syscall (syscall 1)
    // x8 = syscall number (1 = write)
    // x0 = fd (1 = stdout)
    // x1 = buffer pointer
    // x2 = count
    let message = b"[syscall] Hello from SVC!\n";
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #1",           // syscall number: write
            "mov x0, #1",           // fd: stdout
            "mov x1, {buf}",        // buffer
            "mov x2, {len}",        // count
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            buf = in(reg) message.as_ptr(),
            len = in(reg) message.len(),
            ret = out(reg) ret,
            out("x0") _,
            out("x1") _,
            out("x2") _,
            out("x8") _,
        );
    }
    serial_println!("[test] write() returned: {}", ret);

    // Test getpid syscall (syscall 39)
    let pid: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #39",          // syscall number: getpid
            "svc #0",               // syscall!
            "mov {pid}, x0",        // return value
            pid = out(reg) pid,
            out("x8") _,
        );
    }
    serial_println!("[test] getpid() returned: {}", pid);

    // Test clock_gettime syscall (syscall 228)
    let mut timespec: [u64; 2] = [0, 0];
    let clock_ret: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #228",         // syscall number: clock_gettime
            "mov x0, #0",           // CLOCK_REALTIME
            "mov x1, {ts}",         // timespec pointer
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            ts = in(reg) timespec.as_mut_ptr(),
            ret = out(reg) clock_ret,
            out("x0") _,
            out("x1") _,
            out("x8") _,
        );
    }
    if clock_ret == 0 {
        serial_println!("[test] clock_gettime() returned: {}.{:09} seconds", timespec[0], timespec[1]);
    } else {
        serial_println!("[test] clock_gettime() failed with: {}", clock_ret);
    }

    // Test unknown syscall (should return -ENOSYS)
    let enosys: i64;
    unsafe {
        core::arch::asm!(
            "mov x8, #9999",        // invalid syscall number
            "svc #0",               // syscall!
            "mov {ret}, x0",        // return value
            ret = out(reg) enosys,
            out("x8") _,
        );
    }
    serial_println!("[test] unknown syscall returned: {} (expected -38 ENOSYS)", enosys);

    serial_println!("[test] Syscall tests complete!");
}

/// Test userspace execution by transitioning to EL0.
///
/// This creates a minimal ARM64 program in RAM (user-accessible region)
/// that immediately makes a syscall back to the kernel.
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)] // Test function for manual debugging
fn test_userspace() {
    use kernel::arch_impl::aarch64::context;

    // User program code - a minimal program that:
    // 1. Prints a message via write syscall
    // 2. Exits via exit syscall
    //
    // ARM64 assembly (little-endian encoding):
    //   mov x8, #1           // syscall: write
    //   mov x0, #1           // fd: stdout
    //   adr x1, msg          // buffer: message
    //   mov x2, #28          // count: message length
    //   svc #0               // syscall!
    //   mov x8, #0           // syscall: exit
    //   mov x0, #42          // exit code: 42
    //   svc #0               // syscall!
    // msg:
    //   .ascii "[user] Hello from EL0!\n"
    //
    // Note: We need to carefully craft the message reference since adr uses PC-relative
    // addressing. Instead, we'll embed the message address directly.

    #[repr(align(4))]
    #[allow(dead_code)]  // Fields are used via write_volatile
    struct UserProgram {
        code: [u32; 16],
        message: [u8; 32],
    }

    // Place user program in the user-accessible region (0x4100_0000+)
    // This region has AP=0b01, allowing EL0 to read/write/execute
    // (Note: EL1 cannot execute here due to ARM64 implicit PXN with AP=0b01)
    let user_code_addr: u64 = 0x4100_0000;
    let user_stack_top: u64 = 0x4101_0000; // 64KB above code for stack

    // The message is at offset 0x40 (64 bytes) from code start
    // So full address = 0x4100_0000 + 0x40 = 0x4100_0040
    let program = UserProgram {
        code: [
            // Load message address 0x41000040 into x1
            // movz x1, #0x0040    (x1 = 0x40)
            0xd2800801,
            // movk x1, #0x4100, lsl #16    (x1 = 0x41000040)
            0xf2a82001,

            // mov x8, #1 (write syscall)
            0xd2800028,
            // mov x0, #1 (fd = stdout)
            0xd2800020,
            // mov x2, #24 (message length)
            0xd2800302,
            // svc #0
            0xd4000001,

            // mov x8, #0 (exit syscall)
            0xd2800008,
            // mov x0, #42 (exit code)
            0xd2800540,
            // svc #0
            0xd4000001,

            // Just in case exit doesn't work, spin forever
            // b . (branch to self)
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000,
            0x14000000, // 16th element
        ],
        message: *b"[user] Hello from EL0!\n\0\0\0\0\0\0\0\0\0",
    };

    // Copy program to user memory
    unsafe {
        let dst = user_code_addr as *mut UserProgram;
        core::ptr::write_volatile(dst, program);

        // Ensure instruction cache coherency
        // Clean and invalidate data cache, then invalidate instruction cache
        core::arch::asm!(
            "dc cvau, {addr}",        // Clean data cache by VA to PoU
            "dsb ish",                 // Data synchronization barrier
            "ic ivau, {addr}",        // Invalidate instruction cache by VA to PoU
            "dsb ish",                 // Data synchronization barrier
            "isb",                     // Instruction synchronization barrier
            addr = in(reg) user_code_addr,
            options(nostack)
        );
    }

    serial_println!("[test] User program placed at {:#x}", user_code_addr);
    serial_println!("[test] User stack at {:#x}", user_stack_top);
    serial_println!("[test] Transitioning to EL0...");

    // Jump to userspace!
    // Note: return_to_userspace never returns - it uses ERET
    // The user program will exit via syscall, which we handle in exception.rs
    unsafe {
        context::return_to_userspace(user_code_addr, user_stack_top);
    }
}

/// Read current exception level from CurrentEL register
#[cfg(target_arch = "aarch64")]
fn current_exception_level() -> u8 {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, currentel", out(reg) el, options(nomem, nostack));
    }
    ((el >> 2) & 0x3) as u8
}

/// Initialize graphics subsystem
///
/// This initializes the VirtIO GPU and sets up the split-screen terminal UI
/// with graphics demo on the left and terminal on the right.
#[cfg(target_arch = "aarch64")]
fn init_graphics() -> Result<(), &'static str> {
    // Initialize VirtIO GPU driver
    kernel::drivers::virtio::gpu_mmio::init()?;

    arm64_fb::init_shell_framebuffer()?;

    // Get framebuffer dimensions
    let (width, height) = arm64_fb::dimensions().ok_or("Failed to get framebuffer dimensions")?;
    serial_println!("[graphics] Framebuffer: {}x{}", width, height);

    // Calculate layout: 50/50 split with 4-pixel divider
    let divider_width = 4usize;
    let divider_x = width / 2;
    let left_width = divider_x;

    // Get the framebuffer and draw initial frame
    if let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() {
        let mut fb_guard = fb.lock();

        // Clear entire screen with dark background
        fill_rect(
            &mut *fb_guard,
            Rect {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            Color::rgb(15, 20, 35),
        );

        // Draw vertical divider
        let divider_color = Color::rgb(60, 80, 100);
        for i in 0..divider_width {
            draw_vline(&mut *fb_guard, (divider_x + i) as i32, 0, height as i32 - 1, divider_color);
        }

        // Flush to display
        fb_guard.flush();
    }

    // Initialize particle system for left pane (animation will start later)
    // Leave a small margin from edges
    let margin = 10;
    particles::start_animation(
        margin as i32,
        margin as i32,
        (left_width - margin) as i32,
        (height - margin) as i32,
    );
    serial_println!("[graphics] Particle system initialized");

    // Initialize the render queue for deferred framebuffer rendering
    // This enables lock-free echo from interrupt context
    kernel::graphics::render_queue::init();

    // Initialize log capture ring buffer for serial output tee
    kernel::graphics::log_capture::init();

    serial_println!("[graphics] Split-screen terminal UI initialized");
    Ok(())
}

/// Panic handler
#[cfg(target_arch = "aarch64")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!();
    serial_println!("========================================");
    serial_println!("  KERNEL PANIC!");
    serial_println!("========================================");
    serial_println!("{}", info);
    serial_println!();

    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}


// =============================================================================
// Non-aarch64 stub section
// When building for non-aarch64 targets (e.g., x86_64), this binary is just a stub.
// The real x86_64 kernel is in main.rs which provides its own lang items.
// =============================================================================

#[cfg(not(target_arch = "aarch64"))]
mod non_aarch64_stub {
    use core::panic::PanicInfo;

    // Stub panic handler for non-aarch64 builds.
    // The real x86_64 panic handler is in main.rs.
    // This is needed because Cargo compiles all binaries for the target,
    // even if they are gated out with cfg.
    #[panic_handler]
    fn panic(_info: &PanicInfo) -> ! {
        loop {}
    }
}
