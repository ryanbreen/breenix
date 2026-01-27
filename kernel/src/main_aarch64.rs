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
fn run_userspace_from_ext2(path: &str) -> Result<core::convert::Infallible, &'static str> {
    use alloc::string::String;
    use core::arch::asm;
    use kernel::arch_impl::aarch64::context::return_to_userspace;

    let fs_guard = kernel::fs::ext2::root_fs();
    let fs = fs_guard.as_ref().ok_or("ext2 root filesystem not mounted")?;

    let inode_num = fs.resolve_path(path).map_err(|_| "init_shell not found")?;
    let inode = fs.read_inode(inode_num).map_err(|_| "failed to read inode")?;

    if inode.is_dir() {
        return Err("init_shell is a directory");
    }

    let elf_data = fs.read_file_content(&inode).map_err(|_| "failed to read init_shell")?;

    if elf_data.len() < 4 || &elf_data[0..4] != b"\x7fELF" {
        return Err("init_shell is not a valid ELF file");
    }

    let proc_name = path.rsplit('/').next().unwrap_or(path);
    let pid = {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.create_process(String::from(proc_name), &elf_data)?
        } else {
            return Err("process manager not initialized");
        }
    };

    let (entry_point, user_stack_top, ttbr0_phys, main_thread_id, main_thread_clone) = {
        let manager_guard = kernel::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some(process) = manager.get_process(pid) {
                let entry = process.entry_point.as_u64();
                let thread = process
                    .main_thread
                    .as_ref()
                    .ok_or("process has no main thread")?;
                let stack_top = thread.stack_top.as_u64();
                let ttbr0 = process
                    .page_table
                    .as_ref()
                    .ok_or("process has no page table")?
                    .level_4_frame()
                    .start_address()
                    .as_u64();
                (entry, stack_top, ttbr0, thread.id, thread.clone())
            } else {
                return Err("process not found after creation");
            }
        } else {
            return Err("process manager not available");
        }
    };

    // Register the userspace thread with the scheduler as the current running thread.
    // This uses spawn_as_current which:
    // - Adds the thread to scheduler's thread list (for lookups)
    // - Sets it as current_thread
    // - Does NOT add to ready_queue (thread is already running)
    // - Does NOT set need_resched (we don't want preemption until first syscall completes)
    kernel::task::scheduler::spawn_as_current(alloc::boxed::Box::new(main_thread_clone));

    // Set per-CPU pointers to the thread in the scheduler
    kernel::task::scheduler::with_thread_mut(main_thread_id, |thread| {
        let thread_ptr = thread as *mut kernel::task::thread::Thread;
        kernel::per_cpu_aarch64::set_current_thread(thread_ptr);
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            kernel::per_cpu_aarch64::set_kernel_stack_top(kernel_stack_top.as_u64());
        }
    });

    // Mark the process as running.
    {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.set_current_pid(pid);
        }
    }

    unsafe {
        asm!("msr ttbr0_el1, {0}", "isb", in(reg) ttbr0_phys, options(nostack, preserves_flags));
    }
    unsafe { return_to_userspace(entry_point, user_stack_top); }
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
use kernel::graphics::primitives::{draw_vline, fill_rect, Canvas, Color, Rect};
#[cfg(target_arch = "aarch64")]
use kernel::graphics::terminal_manager;
#[cfg(target_arch = "aarch64")]
use kernel::drivers::virtio::input_mmio::{self, event_type};
#[cfg(target_arch = "aarch64")]
use kernel::shell::ShellState;

/// Kernel entry point called from assembly boot code.
#[cfg(target_arch = "aarch64")]
///
/// At this point:
/// - We're running at EL1 (or need to drop from EL2)
/// - Stack is set up
/// - BSS is zeroed
/// - MMU is already enabled by boot.S (high-half kernel)
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // Initialize physical memory offset (needed for MMIO access)
    kernel::memory::init_physical_memory_offset_aarch64();

    // Initialize serial output first so we can print
    serial::init_serial();

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
    // We use 0x42000000..0x50000000 (224MB) for frame allocation
    // Kernel stacks are at 0x51000000..0x52000000 (16MB)
    serial_println!("[boot] Initializing memory management...");
    kernel::memory::frame_allocator::init_aarch64(0x4200_0000, 0x5000_0000);
    kernel::memory::init_aarch64_heap();
    kernel::memory::kernel_stack::init();
    serial_println!("[boot] Memory management ready");

    // Initialize timer
    serial_println!("[boot] Initializing Generic Timer...");
    timer::calibrate();
    let freq = timer::frequency_hz();
    serial_println!("[boot] Timer frequency: {} Hz ({} MHz)", freq, freq / 1_000_000);

    // Read current timestamp
    let ts = timer::rdtsc();
    serial_println!("[boot] Current timestamp: {}", ts);

    // Initialize GIC
    serial_println!("[boot] Initializing GICv2...");
    Gicv2::init();
    serial_println!("[boot] GIC initialized");

    // Enable UART receive interrupt (IRQ 33 = SPI 1)
    serial_println!("[boot] Enabling UART interrupts...");

    // Enable IRQ 33 in GIC (PL011 UART)
    serial_println!("[boot] Enabling GIC IRQ 33 (UART0)...");
    Gicv2::enable_irq(33); // UART0 IRQ

    // Enable RX interrupts in PL011
    serial::enable_rx_interrupt();

    serial_println!("[boot] UART interrupts enabled");

    // Enable interrupts
    serial_println!("[boot] Enabling interrupts...");
    unsafe { Aarch64Cpu::enable_interrupts(); }
    let irq_enabled = Aarch64Cpu::interrupts_enabled();
    serial_println!("[boot] Interrupts enabled: {}", irq_enabled);

    // Initialize device drivers (VirtIO MMIO enumeration)
    serial_println!("[boot] Initializing device drivers...");
    let device_count = kernel::drivers::init();
    serial_println!("[boot] Found {} devices", device_count);

    // Initialize network stack (after VirtIO network driver is ready)
    serial_println!("[boot] Initializing network stack...");
    kernel::net::init();

    // Initialize filesystem layer (requires VirtIO block device)
    serial_println!("[boot] Initializing filesystem...");

    // Initialize ext2 root filesystem (if block device present)
    match kernel::fs::ext2::init_root_fs() {
        Ok(()) => serial_println!("[boot] ext2 root filesystem mounted"),
        Err(e) => serial_println!("[boot] ext2 init: {} (continuing without root fs)", e),
    }

    // Initialize devfs (/dev virtual filesystem)
    kernel::fs::devfs::init();
    serial_println!("[boot] devfs initialized at /dev");

    // Initialize devptsfs (/dev/pts pseudo-terminal slave filesystem)
    kernel::fs::devptsfs::init();
    serial_println!("[boot] devptsfs initialized at /dev/pts");

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

    // Initialize timer interrupt for preemptive scheduling
    // This MUST come after per-CPU data and scheduler are initialized
    serial_println!("[boot] Initializing timer interrupt...");
    timer_interrupt::init();
    serial_println!("[boot] Timer interrupt initialized");

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Boot Complete!");
    serial_println!("========================================");
    serial_println!();
    serial_println!("Hello from ARM64!");
    serial_println!();

    // Try to load and run userspace init_shell from ext2 or test disk
    if device_count > 0 {
        serial_println!("[boot] Loading userspace init_shell from ext2...");
        match run_userspace_from_ext2("/bin/init_shell") {
            Err(e) => {
                serial_println!("[boot] Failed to load init_shell from ext2: {}", e);
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

    // Write welcome message to the terminal (right pane)
    terminal_manager::write_str_to_shell("Breenix ARM64 Interactive Shell\n");
    terminal_manager::write_str_to_shell("================================\n\n");
    terminal_manager::write_str_to_shell("Type 'help' for available commands.\n\n");
    terminal_manager::write_str_to_shell("breenix> ");

    serial_println!("[interactive] Entering interactive mode");
    serial_println!("[interactive] Input via VirtIO keyboard");
    serial_println!();

    // Create shell state for command processing
    let mut shell = ShellState::new();

    // Check if we have graphics (VirtIO GPU) or running in serial-only mode
    let has_graphics = kernel::graphics::arm64_fb::SHELL_FRAMEBUFFER.get().is_some();
    if !has_graphics {
        serial_println!("[interactive] Running in serial-only mode (no VirtIO GPU)");
        serial_println!("[interactive] Type commands at the serial console");
        serial_println!();
        serial_print!("breenix> ");
    }

    // Poll for VirtIO keyboard and serial input
    let mut shift_pressed = false;

    loop {

        // Poll serial input (for -nographic mode)
        // This is the primary input method when no VirtIO keyboard is available
        while let Some(byte) = kernel::serial_aarch64::get_received_byte() {
            // Handle special keys
            let c = match byte {
                // Backspace
                0x7F | 0x08 => '\x08',
                // Enter (CR)
                0x0D => '\n',
                // Tab
                0x09 => '\t',
                // Escape sequences start with 0x1B - for now, ignore
                0x1B => continue,
                // Regular ASCII
                b if b >= 0x20 && b < 0x7F => byte as char,
                // Ctrl+C
                0x03 => {
                    serial_println!("^C");
                    continue;
                }
                // Other control characters
                _ => continue,
            };

            // In serial-only mode, echo to serial and process through shell
            if !has_graphics {
                // Process through shell (which outputs to terminal_manager, but that won't work)
                // Instead, handle serial shell inline
                process_serial_shell_char(&mut shell, c);
            } else {
                // With graphics, route to terminal manager
                shell.process_char(c);
            }
        }

        // Poll VirtIO input device for keyboard events (when GPU is available)
        if input_mmio::is_initialized() {
            for event in input_mmio::poll_events() {
                // Only process key events (EV_KEY = 1)
                if event.event_type == event_type::EV_KEY {
                    let keycode = event.code;
                    let pressed = event.value != 0;

                    // Track shift key state
                    if input_mmio::is_shift(keycode) {
                        shift_pressed = pressed;
                        continue;
                    }

                    // Only process key presses (not releases)
                    if pressed {
                        // Convert keycode to character
                        if let Some(c) = input_mmio::keycode_to_char(keycode, shift_pressed) {
                            serial_println!("[key] code={} char='{}'", keycode, c);
                            // Pass character to shell for processing
                            shell.process_char(c);
                        } else if !input_mmio::is_modifier(keycode) {
                            // Unknown non-modifier key
                            serial_println!("[key] code={} (no mapping)", keycode);
                        }
                    }
                }
            }
        }

        // Wait for interrupt instead of busy-spinning to save CPU
        // WFI will wake on any interrupt (timer, UART RX, etc.)
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Process a character for the serial-only shell mode.
///
/// This is used when running without graphics (-nographic) where output
/// goes to serial instead of the graphical terminal.
#[cfg(target_arch = "aarch64")]
fn process_serial_shell_char(shell: &mut ShellState, c: char) {
    match c {
        '\n' | '\r' => {
            // Echo newline
            serial_println!();
            // Execute command (shell writes to terminal_manager which won't render,
            // but we can capture the command and execute it ourselves)
            execute_serial_command(shell);
            // Show new prompt
            serial_print!("breenix> ");
        }
        '\x08' | '\x7f' => {
            // Backspace
            if shell.line_pos() > 0 {
                shell.backspace();
                // Echo backspace sequence to erase character
                serial_print!("\x08 \x08");
            }
        }
        c if c.is_ascii() && !c.is_control() => {
            // Regular printable character - add to buffer and echo
            if shell.add_char(c) {
                serial_print!("{}", c);
            }
        }
        _ => {}
    }
}

/// Execute the command in the shell's line buffer (serial mode).
#[cfg(target_arch = "aarch64")]
fn execute_serial_command(shell: &mut ShellState) {
    use kernel::arch_impl::aarch64::timer;

    let line = shell.get_line();
    let line = line.trim();

    if line.is_empty() {
        return;
    }

    // Parse command and arguments
    let (cmd, args) = match line.find(' ') {
        Some(pos) => (&line[..pos], line[pos + 1..].trim()),
        None => (line, ""),
    };

    match cmd {
        "help" => {
            serial_println!("========================================");
            serial_println!("Breenix ARM64 Serial Shell");
            serial_println!("========================================");
            serial_println!();
            serial_println!("Commands:");
            serial_println!("  help     - Show this help message");
            serial_println!("  echo     - Print arguments");
            serial_println!("  uptime   - Show time since boot");
            serial_println!("  uname    - Show system information");
            serial_println!("  ps       - List running processes");
            serial_println!("  mem      - Show memory usage");
            serial_println!();
            serial_println!("Press Ctrl-A X to exit QEMU.");
            serial_println!();
        }
        "echo" => {
            serial_println!("{}", args);
        }
        "clear" => {
            // Send ANSI clear screen sequence
            serial_print!("\x1b[2J\x1b[H");
        }
        "uptime" | "time" => {
            match timer::monotonic_time() {
                Some((secs, nanos)) => {
                    let hours = secs / 3600;
                    let mins = (secs % 3600) / 60;
                    let secs_rem = secs % 60;
                    let millis = nanos / 1_000_000;
                    serial_print!("up ");
                    if hours > 0 {
                        serial_print!("{} hour{}, ", hours, if hours == 1 { "" } else { "s" });
                    }
                    if mins > 0 || hours > 0 {
                        serial_print!("{} minute{}, ", mins, if mins == 1 { "" } else { "s" });
                    }
                    serial_println!("{}.{:03} second{}", secs_rem, millis,
                        if secs_rem == 1 && millis == 0 { "" } else { "s" });
                }
                None => {
                    serial_println!("Error: timer not available");
                }
            }
        }
        "uname" => {
            serial_println!("Breenix 0.1.0 aarch64 ARM Cortex-A72");
        }
        "ps" => {
            serial_println!("  PID  STATE  NAME");
            serial_println!("    0  R      kernel");
            serial_println!("    1  R      shell");
        }
        "mem" | "free" => {
            serial_println!("Memory usage:");
            serial_println!("  Total RAM:   512 MB (QEMU virt machine)");
            serial_println!("  Kernel heap: 256 KB pre-allocated");
            serial_println!("  Allocator:   bump allocator (ARM64)");
        }
        "exit" | "quit" => {
            serial_println!("Cannot exit kernel shell!");
            serial_println!("Press Ctrl-A X to exit QEMU.");
        }
        _ => {
            serial_println!("Unknown command: {}", cmd);
            serial_println!("Type 'help' for available commands.");
        }
    }

    // Clear the line buffer for next command
    shell.clear_line();
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

    // Use a dummy stack address for the idle task (we're already running on a stack)
    let dummy_stack_top = VirtAddr::new(0x4000_0000);
    let dummy_stack_bottom = VirtAddr::new(0x3FFF_0000);
    let dummy_tls = VirtAddr::zero();

    // Create the idle task (thread ID 0)
    let mut idle_task = Box::new(Thread::new(
        String::from("swapper/0"),  // Linux convention: swapper/0 is the idle task
        idle_thread_fn,
        dummy_stack_top,
        dummy_stack_bottom,
        dummy_tls,
        ThreadPrivilege::Kernel,
    ));

    // Mark as running with ID 0
    idle_task.state = ThreadState::Running;
    idle_task.id = 0;

    // Set up per-CPU current thread pointer
    let idle_task_ptr = &*idle_task as *const _ as *mut Thread;
    per_cpu_aarch64::set_current_thread(idle_task_ptr);

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

    // Initialize the shell framebuffer (this is what terminal_manager uses)
    arm64_fb::init_shell_framebuffer()?;

    // Get framebuffer dimensions
    let (width, height) = arm64_fb::dimensions().ok_or("Failed to get framebuffer dimensions")?;
    serial_println!("[graphics] Framebuffer: {}x{}", width, height);

    // Calculate layout: 50/50 split with 4-pixel divider
    let divider_width = 4usize;
    let divider_x = width / 2;
    let left_width = divider_x;
    let right_x = divider_x + divider_width;
    let right_width = width.saturating_sub(right_x);

    // Get the framebuffer and draw
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
            Color::rgb(20, 30, 50),
        );

        // Draw graphics demo on left pane
        draw_graphics_demo(&mut *fb_guard, 0, 0, left_width, height);

        // Draw vertical divider
        let divider_color = Color::rgb(60, 80, 100);
        for i in 0..divider_width {
            draw_vline(&mut *fb_guard, (divider_x + i) as i32, 0, height as i32 - 1, divider_color);
        }

        // Flush to display
        fb_guard.flush();
    }

    // Initialize terminal manager for the right side
    terminal_manager::init_terminal_manager(right_x, 0, right_width, height);

    // Initialize the terminal manager UI
    if let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() {
        let mut fb_guard = fb.lock();
        if let Some(mut mgr) = terminal_manager::TERMINAL_MANAGER.try_lock() {
            if let Some(manager) = mgr.as_mut() {
                manager.init(&mut *fb_guard);
            }
        }
        // Flush after terminal init
        fb_guard.flush();
    }

    serial_println!("[graphics] Split-screen terminal UI initialized");
    Ok(())
}

/// Draw a graphics demo on the left pane
#[cfg(target_arch = "aarch64")]
fn draw_graphics_demo(canvas: &mut impl Canvas, x: usize, y: usize, width: usize, height: usize) {
    let padding = 20;

    // Title area
    let title_y = y + padding;

    // Draw title background
    fill_rect(
        canvas,
        Rect {
            x: (x + padding) as i32,
            y: title_y as i32,
            width: (width - padding * 2) as u32,
            height: 40,
        },
        Color::rgb(40, 60, 80),
    );

    // Draw colored rectangles as demo
    let box_width = 120;
    let box_height = 80;
    let box_y = y + 100;
    let box_spacing = 20;

    // Red box
    fill_rect(
        canvas,
        Rect {
            x: (x + padding) as i32,
            y: box_y as i32,
            width: box_width,
            height: box_height,
        },
        Color::RED,
    );

    // Green box
    fill_rect(
        canvas,
        Rect {
            x: (x + padding + box_width as usize + box_spacing) as i32,
            y: box_y as i32,
            width: box_width,
            height: box_height,
        },
        Color::GREEN,
    );

    // Blue box
    fill_rect(
        canvas,
        Rect {
            x: (x + padding) as i32,
            y: (box_y + box_height as usize + box_spacing) as i32,
            width: box_width,
            height: box_height,
        },
        Color::BLUE,
    );

    // Yellow box
    fill_rect(
        canvas,
        Rect {
            x: (x + padding + box_width as usize + box_spacing) as i32,
            y: (box_y + box_height as usize + box_spacing) as i32,
            width: box_width,
            height: box_height,
        },
        Color::rgb(255, 255, 0), // Yellow
    );

    // Draw some gradient bars at the bottom
    let bar_y = y + height - 100;
    let bar_height = 20;
    for i in 0..width.saturating_sub(padding * 2) {
        let intensity = ((i * 255) / (width - padding * 2)) as u8;
        let color = Color::rgb(intensity, intensity, intensity);
        fill_rect(
            canvas,
            Rect {
                x: (x + padding + i) as i32,
                y: bar_y as i32,
                width: 1,
                height: bar_height,
            },
            color,
        );
    }

    // Draw color bars
    let colors = [
        Color::RED,
        Color::GREEN,
        Color::BLUE,
        Color::rgb(0, 255, 255),   // Cyan
        Color::rgb(255, 0, 255),   // Magenta
        Color::rgb(255, 255, 0),   // Yellow
    ];
    let color_bar_y = bar_y + bar_height as usize + 10;
    let color_bar_width = (width - padding * 2) / colors.len();
    for (i, &color) in colors.iter().enumerate() {
        fill_rect(
            canvas,
            Rect {
                x: (x + padding + i * color_bar_width) as i32,
                y: color_bar_y as i32,
                width: color_bar_width as u32,
                height: bar_height,
            },
            color,
        );
    }
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
