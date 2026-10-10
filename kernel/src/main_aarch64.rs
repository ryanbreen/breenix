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

/// Read the init ELF binary from ext2 while still single-CPU.
///
/// This must be called BEFORE SMP bring-up because AHCI DMA reads fail when
/// secondary CPUs are online (PCI interconnect interference from their GIC
/// timer interrupt activity). Returns the raw ELF bytes on success.
#[cfg(target_arch = "aarch64")]
#[cfg_attr(
    any(
        feature = "kthread_test_only",
        feature = "kthread_stress_test",
        feature = "workqueue_test_only"
    ),
    allow(dead_code)
)]
fn read_init_from_ext2(path: &str) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let fs_guard = kernel::fs::ext2::root_fs_read();
    let fs = fs_guard
        .as_ref()
        .ok_or("ext2 root filesystem not mounted")?;

    let inode_num = fs.resolve_path(path).map_err(|_| "init not found")?;

    let inode = fs
        .read_inode(inode_num)
        .map_err(|_| "failed to read inode")?;

    if inode.is_dir() {
        unsafe {
            kernel::arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts();
        }
        return Err("init is a directory");
    }

    let elf_data = fs
        .read_file_content_coherent(inode_num, &inode)
        .map_err(|_| "failed to read init")?;

    drop(fs_guard);

    Ok(elf_data)
}

/// Which PID 1 a production boot launches. QEMU selects it with
/// `-fw_cfg name=opt/breenix/mode,string=<mode>`; absent or unknown is `Default`.
/// `program` also needs `-fw_cfg name=opt/breenix/program,string=<absolute path>`,
/// and `suite` needs `-fw_cfg name=opt/breenix/suite,string=<id>`. With no fw_cfg
/// mode (including the VM runner configuration), the boot-target file
/// (`kernel::boot::target`) can select a suite or probe.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum BootMode {
    Default,
    Probe,
    Shell,
    Desktop,
    Program,
    Suite,
}

#[cfg(target_arch = "aarch64")]
impl BootMode {
    fn name(self) -> &'static str {
        match self {
            BootMode::Default => "default",
            BootMode::Probe => "probe",
            BootMode::Shell => "shell",
            BootMode::Desktop => "desktop",
            BootMode::Program => "program",
            BootMode::Suite => "suite",
        }
    }
}

/// Read the boot mode: fw_cfg first, then the boot-target file, else `default`. An
/// unknown request, a program request without an absolute program path, a suite
/// request without a valid suite id, an unusable boot-target file, or any request to
/// a `testing` kernel (which always runs its test loader), runs `default` and is
/// noted on a separate line. Returns the program path for program mode and the suite
/// id for suite mode. `announce_boot_mode` prints the mode once it is known to run.
#[cfg(target_arch = "aarch64")]
fn read_boot_mode() -> (BootMode, Option<alloc::string::String>) {
    use kernel::boot::target;
    // fw_cfg exists only on QEMU (its MMIO window is absent on Parallels).
    let requested = if kernel::platform_config::is_qemu() {
        kernel::drivers::fw_cfg::read_string("opt/breenix/mode")
    } else {
        None
    };
    let mode = match requested.as_deref() {
        Some("probe") => BootMode::Probe,
        Some("shell") => BootMode::Shell,
        Some("desktop") => BootMode::Desktop,
        Some("program") => BootMode::Program,
        Some("suite") => BootMode::Suite,
        _ => BootMode::Default,
    };
    // Without a fw_cfg mode, the boot-target file may name a suite or probe.
    let mut target_suite = None;
    let mode = match requested.as_deref() {
        None | Some("") => match target::read() {
            Ok(Some(target::Target::Probe)) => BootMode::Probe,
            Ok(Some(target::Target::Suite(id))) => {
                target_suite = Some(id);
                BootMode::Suite
            }
            Ok(None) => BootMode::Default,
            Err(why) => {
                serial_println!("[boot] Ignoring boot target {}: {}", target::PATH, why);
                BootMode::Default
            }
        },
        Some(other) if mode == BootMode::Default && other != "default" => {
            serial_println!("[boot] Ignoring unknown boot mode {:?}", other);
            BootMode::Default
        }
        _ => mode,
    };
    let mode = if cfg!(feature = "testing") && mode != BootMode::Default {
        serial_println!(
            "[boot] Ignoring boot mode {:?}: the testing kernel runs its test loader",
            mode.name()
        );
        BootMode::Default
    } else {
        mode
    };
    let arg = match mode {
        BootMode::Program => match kernel::drivers::fw_cfg::read_string("opt/breenix/program") {
            Some(path) if path.starts_with('/') => Some(path),
            Some(path) if !path.is_empty() => {
                serial_println!(
                    "[boot] Ignoring boot mode \"program\": program path {:?} is not absolute",
                    path
                );
                None
            }
            _ => {
                serial_println!(
                    "[boot] Ignoring boot mode \"program\": no program given (-fw_cfg name=opt/breenix/program)"
                );
                None
            }
        },
        BootMode::Suite if target_suite.is_some() => target_suite,
        BootMode::Suite => match kernel::drivers::fw_cfg::read_string("opt/breenix/suite") {
            Some(id) if target::is_suite_id(&id) => Some(id),
            Some(id) if !id.is_empty() => {
                serial_println!(
                    "[boot] Ignoring boot mode \"suite\": {:?} is not a suite id",
                    id
                );
                None
            }
            _ => {
                serial_println!(
                    "[boot] Ignoring boot mode \"suite\": no suite given (-fw_cfg name=opt/breenix/suite)"
                );
                None
            }
        },
        _ => None,
    };
    let mode = if matches!(mode, BootMode::Program | BootMode::Suite) && arg.is_none() {
        BootMode::Default
    } else {
        mode
    };
    (mode, arg)
}

/// Print the one `[boot] Boot mode: <mode>` line, naming the mode that actually
/// runs (`program <path>` for program mode, `suite <id>` for suite mode), and show
/// it on the boot screen.
#[cfg(target_arch = "aarch64")]
fn announce_boot_mode(mode: BootMode, arg: Option<&str>) {
    match (mode, arg) {
        (BootMode::Program | BootMode::Suite, Some(arg)) => {
            serial_println!("[boot] Boot mode: {} {}", mode.name(), arg);
            boot_screen::set_mode(format_args!("{} {}", mode.name(), arg));
        }
        _ => {
            serial_println!("[boot] Boot mode: {}", mode.name());
            if cfg!(feature = "testing") {
                boot_screen::set_mode(format_args!("tests (testing kernel)"));
            } else {
                boot_screen::set_mode(format_args!("{}", mode.name()));
            }
        }
    }
}

/// Whether `-fw_cfg name=opt/breenix/fbconsole,string=log` asks for kernel log
/// lines on the screen. Anything else keeps the boot screen, noting a bad value.
#[cfg(target_arch = "aarch64")]
fn read_fbconsole_log() -> bool {
    if !kernel::platform_config::is_qemu() {
        return false;
    }
    match kernel::drivers::fw_cfg::read_string("opt/breenix/fbconsole").as_deref() {
        Some("log") => {
            serial_println!("[boot] fbconsole: log (kernel log lines on screen)");
            true
        }
        None | Some("") => false,
        Some(other) => {
            serial_println!(
                "[boot] Ignoring unknown fbconsole {:?} (expected log)",
                other
            );
            false
        }
    }
}

/// The program a boot mode launches as PID 1, and its argv.
#[cfg(target_arch = "aarch64")]
struct InitLaunch {
    path: alloc::string::String,
    argv: alloc::vec::Vec<alloc::vec::Vec<u8>>,
}

#[cfg(target_arch = "aarch64")]
impl InitLaunch {
    fn new(path: &str, argv: &[&[u8]]) -> InitLaunch {
        InitLaunch {
            path: alloc::string::String::from(path),
            argv: argv.iter().map(|arg| arg.to_vec()).collect(),
        }
    }

    fn default_init() -> InitLaunch {
        InitLaunch::new("/sbin/init", &[b"/sbin/init"])
    }

    fn for_mode(mode: BootMode, arg: Option<&str>) -> InitLaunch {
        match mode {
            BootMode::Default => InitLaunch::default_init(),
            BootMode::Probe => InitLaunch::new("/sbin/probe", &[b"/sbin/probe"]),
            BootMode::Shell => InitLaunch::new("/sbin/init", &[b"init", b"shell"]),
            BootMode::Desktop => InitLaunch::new("/sbin/init", &[b"init", b"desktop"]),
            BootMode::Program => InitLaunch::new(
                "/sbin/probe",
                &[b"probe", b"--run", arg.unwrap_or("").as_bytes()],
            ),
            BootMode::Suite => {
                let path = kernel::boot::target::suite_path(arg.unwrap_or(""));
                InitLaunch::new(&path, &[path.as_bytes()])
            }
        }
    }

    /// Launch this program from its pre-loaded ELF (see `launch_init_from_elf`).
    fn launch(
        &self,
        elf_data: alloc::vec::Vec<u8>,
    ) -> Result<core::convert::Infallible, &'static str> {
        let argv: alloc::vec::Vec<&[u8]> = self.argv.iter().map(|arg| arg.as_slice()).collect();
        let mut shown = self.path.clone();
        for arg in self.argv.iter().skip(1) {
            shown.push(' ');
            shown.push_str(core::str::from_utf8(arg).unwrap_or("?"));
        }
        boot_screen::stage(Stage::StartingPid1);
        boot_screen::set_status(format_args!("Starting {}", shown));
        launch_init_from_elf(elf_data, &self.path, &argv)
    }
}

/// Release the preempt pin kernel_main takes after per-CPU init, once the boot
/// sequence no longer needs CPU 0 to stay on its boot stack, and publish CPU
/// 0's softirq daemon, which is held back until then so it is not left queued
/// through the unschedulable boot phase.
#[cfg(target_arch = "aarch64")]
fn release_boot_preempt_pin() {
    kernel::per_cpu_aarch64::preempt_enable();
    kernel::task::softirqd::init_online_daemons();
}

/// Create a userspace process from a pre-loaded ELF and jump to it.
///
/// Takes ELF bytes that were read earlier (e.g., before SMP bring-up) and
/// completes the process-creation and userspace-entry sequence.
#[cfg(target_arch = "aarch64")]
#[cfg_attr(
    any(
        feature = "kthread_test_only",
        feature = "kthread_stress_test",
        feature = "workqueue_test_only"
    ),
    allow(dead_code)
)]
fn launch_init_from_elf(
    elf_data: alloc::vec::Vec<u8>,
    path: &str,
    argv: &[&[u8]],
) -> Result<core::convert::Infallible, &'static str> {
    use alloc::string::String;
    use kernel::arch_impl::aarch64::context::return_to_userspace;

    // Disable interrupts for the entire process setup.
    // A pending timer interrupt would fire immediately on enable_interrupts(),
    // context-switch the boot thread away before it registers with the scheduler,
    // and it would never be scheduled back. Interrupts are re-enabled just before
    // return_to_userspace() below via the SPSR loaded by ERET.
    unsafe {
        kernel::arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts();
    }

    if elf_data.len() < 4 || &elf_data[0..4] != b"\x7fELF" {
        unsafe {
            kernel::arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts();
        }
        return Err("init is not a valid ELF file");
    }

    let proc_name = path.rsplit('/').next().unwrap_or(path);

    let (pid, init_thread, designated_pid_raw, reserved_collisions) = {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            let ticket =
                manager.create_init_process_with_argv(String::from(proc_name), &elf_data, argv)?;
            let publication = manager.designate_init(ticket)?;
            let pid = publication.pid();
            let init_thread = manager.publish_init(publication);
            let designated_pid_raw = manager
                .designated_init()
                .ok_or("init designation missing after publication")?
                .as_u64();
            let reserved_collisions =
                kernel::tracing::providers::teardown::init_reserved_pid_collisions_total();
            (pid, init_thread, designated_pid_raw, reserved_collisions)
        } else {
            return Err("process manager not initialized");
        }
    };
    kernel::serial_println!(
        "[INIT_DESIGNATION:aarch64:designated_pid={}:reserved_collisions={}]",
        designated_pid_raw,
        reserved_collisions
    );

    // Advance test stage to ProcessContext - a user process now exists with an fd_table
    // This allows tests that need process context (like sys_socket) to run
    #[cfg(feature = "boot_tests")]
    {
        let failures = kernel::test_framework::advance_to_stage(
            kernel::test_framework::TestStage::ProcessContext,
        );
        if failures > 0 {
            kernel::serial_println!("[boot_tests] {} ProcessContext test(s) failed", failures);
        }
    }

    let (entry_point, user_sp, ttbr0_phys, main_thread_id) = {
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
                (entry, sp, ttbr0, thread.id)
            } else {
                return Err("process not found after creation");
            }
        } else {
            return Err("process manager not available");
        }
    };

    // Re-enable preemption now that the boot sequence is complete.
    // It was disabled in kernel_main after per-CPU init to prevent the scheduler
    // from context-switching the boot CPU away from its boot stack (which would
    // leave the boot CPU stuck in idle_loop_arm64 and never return here).
    // From this point on, the init thread is being set as CPU 0's current thread,
    // and the scheduler can run normally. IRQs remain masked through ERET.
    release_boot_preempt_pin();

    // Register the userspace thread with the scheduler as the current running thread.
    kernel::task::scheduler::spawn_as_current(init_thread);
    kernel::drivers::ahci::emit_polling_attribution_once_if_scheduler_ready();

    // CRITICAL: Reset ALL idle threads' saved contexts to point to idle_loop_arm64.
    // Timer interrupts during boot may have saved idle threads' ELR pointing to
    // kernel_main or other boot code. Without this, when a CPU dispatches its idle
    // thread, it ERETSs to the stale ELR (which could be 0x0 or a boot address).
    //
    // We ask the scheduler which threads are idle threads (one per CPU) and reset
    // only those. This is robust to any number of kthreads or render threads created
    // before secondary CPUs come online, because the thread IDs are not predictable
    // by arithmetic alone.
    //
    // We must NOT reset the init main thread's context. Corrupting init's spsr_el1
    // to EL1h (0x5) causes dispatch_thread_locked to terminate it immediately via
    // the safety guard (spsr & 0xF != 0 → not EL0t → not a valid userspace thread).
    {
        let idle_loop_addr =
            kernel::arch_impl::aarch64::context_switch::idle_loop_arm64 as *const () as u64;
        // Collect idle thread IDs from the scheduler's per-CPU state.
        // cpu_state[cpu].idle_thread is set by init_scheduler and register_cpu_idle_thread.
        let mut idle_tids = [0u64; kernel::arch_impl::aarch64::constants::MAX_CPUS];
        let cpus_online = kernel::arch_impl::aarch64::smp::cpus_online() as usize;
        let idle_count =
            kernel::task::scheduler::collect_idle_thread_ids(&mut idle_tids[..cpus_online]);
        // Reset each idle thread's saved context.
        for i in 0..idle_count {
            let tid = idle_tids[i];
            kernel::task::scheduler::with_thread_mut(tid, |idle_thread| {
                idle_thread.context.elr_el1 = idle_loop_addr;
                idle_thread.context.spsr_el1 = 0x5; // EL1h, DAIF clear
                idle_thread.context.x30 = 0;
            });
        }
        serial_println!(
            "[boot] Reset {} idle thread contexts (CPUs online: {})",
            idle_count,
            cpus_online
        );
    }

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

    // Mark the process as running.
    {
        let mut manager_guard = kernel::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.set_current_pid(pid);
        }
    }

    // Create TLB pressure before switching TTBR0 to the process page table.
    // Touch many HHDM pages to help evict boot identity map entries from the
    // TLB. The subsequent TLBI will do a full invalidation, but warming the
    // HHDM entries in the TLB is still beneficial for early process execution.
    // Interrupts are already disabled at this point (since step 'd').
    for i in 0..4096u64 {
        let addr = (0xFFFF_0000_4000_0000u64 + i * 4096) as *const u8;
        unsafe {
            core::ptr::read_volatile(addr);
        }
    }

    // Switch to process page table with ASID=1 tagging. The TLB pressure
    // above has evicted stale entries from the boot identity map. ASID=1
    // ensures any remaining stale boot entries (ASID=0) don't match.
    //
    // R157/ASID-05: this site used to spell the tag itself, as
    // `ttbr0_phys | (1u64 << 48)`. It no longer does. `adopt_process_ttbr0`
    // normalises what it is handed -- clearing bits [63:48] before setting the
    // userspace ASID -- so an or-only tag here was both redundant and a second
    // spelling of a field the discipline owns.
    // claim-lint:ok: 0 of 0 constructions of the userspace ASID tag remain
    // outside the discipline's own file; the census is
    // `the_asid_tag_is_constructed_in_one_place` in
    // `tests/ttbr0_shadow_reconciliation_structure.rs`
    let ttbr0_value = ttbr0_phys;
    //
    // Install through the shared discipline rather than a raw `msr`: init is
    // the one thread that reaches EL0 without passing through
    // `dispatch_thread_locked`, so nothing else on this path reconciles the
    // per-CPU TTBR0 shadows with the register. `setup_idle_return_locked`
    // publishes the KERNEL root into `next_cr3` on every idle dispatch, the
    // syscall return corridor reads that word FIRST, and nothing on the idle
    // return corridor consumes it -- so a `next_cr3` armed before this point
    // decides where init's first `svc` returns.
    // claim-lint:ok: 9 of the 10 censused process-root installs are routed
    // through the discipline, the 10th being the Tier-1 site; the census is
    // tests/ttbr0_shadow_reconciliation_structure.rs.
    //
    // On this branch's tree the boot CPU is pinned for the whole boot, so it
    // takes no idle dispatch before this call and the word is not observed
    // armed here: this install is DEFENSIVE at this site. The same install
    // shape was measured firing on fix/562-761-aarch64-testing-profile, whose
    // boot sequence hands the loader to a kernel thread and does let the boot
    // CPU idle first: 13 of 26 baseline boots there aborted at init's own
    // return address with FAR=ELR and ESR 0x8200000e, 0 of 24 did with the two
    // shadow stores added, and 4 of 8 did again on reversion (#786).
    // claim-lint:ok: the A/B/A rows are the committed arithmetic of
    // docs/planning/green-program/aarch64-testing/serials/r7/aba/CLASSIFICATION.tsv
    // at fix/562-761-aarch64-testing-profile commit 1245c64b, restated in
    // docs/planning/green-program/aarch64-testing/TTBR0-SHADOW-SLICE-2026-09-04.md
    kernel::arch_impl::aarch64::ttbr0::adopt_process_ttbr0(ttbr0_value);
    // DO NOT call enable_interrupts() here. Interrupts are currently disabled
    // (since step 'd'). The ERET in return_to_userspace() loads SPSR_EL1=0
    // into PSTATE, which has DAIF clear (interrupts enabled). The pending
    // timer interrupt will fire immediately after ERET in EL0 context,
    // entering via the Lower EL IRQ vector. This is correct because:
    // - SP_EL1 is set to kernel_stack_top by return_to_userspace()
    // - The init thread is registered with the scheduler
    // - Per-CPU pointers are configured
    // - TTBR0 is set to the process page table
    //
    // If we enabled interrupts HERE (before ERET), the pending timer would
    // fire in EL1 context, the scheduler would context-switch the boot thread
    // away (thinking it's the init process), and it would never reach ERET.
    unsafe {
        return_to_userspace(entry_point, user_sp);
    }
}

#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::cpu::Aarch64Cpu;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::gic::{self, Gicv2};
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::timer;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::aarch64::timer_interrupt;
#[cfg(target_arch = "aarch64")]
use kernel::arch_impl::traits::{CpuOps, InterruptController};
#[cfg(target_arch = "aarch64")]
use kernel::drivers::virtio::input_mmio;
#[cfg(target_arch = "aarch64")]
use kernel::graphics::arm64_fb;
#[cfg(target_arch = "aarch64")]
use kernel::graphics::boot_screen::{self, Stage};
#[cfg(target_arch = "aarch64")]
use kernel::graphics::particles;
#[cfg(target_arch = "aarch64")]
use kernel::serial;

// Enter Rust only after selecting the linked code mapping. The loader's SP is
// already in its low linked alias (0x42000000, IPA 0x82000000 on VMware).
// QEMU's boot.S has already selected both high-half code and stack addresses.
// No Rust frame or callee-saved address survives this non-returning transition.
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    .section .text.kernel_entry, "ax"
    .global kernel_main
    .type kernel_main, %function
kernel_main:
    cbz x0, 1f
    mov x8, #0xffff000000000000
    add sp, sp, x8
1:
    movz x9, #:abs_g0_nc:kernel_main_linked
    movk x9, #:abs_g1_nc:kernel_main_linked
    movk x9, #:abs_g2_nc:kernel_main_linked
    movk x9, #:abs_g3:kernel_main_linked
    br x9
    .size kernel_main, . - kernel_main
    "#
);

#[cfg(target_arch = "aarch64")]
#[export_name = "kernel_main_linked"]
#[inline(never)]
#[cfg_attr(feature = "kthread_test_only", allow(unreachable_code))]
pub extern "C" fn kernel_main(hw_config_ptr: u64) -> ! {
    // One-time check immediately after the mapping transition: ADRP must now
    // agree with absolute linker relocations, including on relocated VMware RAM.
    let relative_text: u64;
    let absolute_text: u64;
    unsafe {
        core::arch::asm!(
            "adrp {relative}, __kernel_text_start",
            "add {relative}, {relative}, :lo12:__kernel_text_start",
            "movz {absolute}, #:abs_g0_nc:__kernel_text_start",
            "movk {absolute}, #:abs_g1_nc:__kernel_text_start",
            "movk {absolute}, #:abs_g2_nc:__kernel_text_start",
            "movk {absolute}, #:abs_g3:__kernel_text_start",
            relative = out(reg) relative_text,
            absolute = out(reg) absolute_text,
            options(nostack, nomem, preserves_flags),
        );
    }
    if relative_text != absolute_text && hw_config_ptr != 0 {
        // Configure the UART on this failure path so the panic is readable on
        // VMware too, before normal platform initialization has taken place.
        let config = unsafe { &*(hw_config_ptr as *const kernel::platform_config::HardwareConfig) };
        kernel::platform_config::init_from_parallels(config);
    }
    assert_eq!(
        relative_text, absolute_text,
        "ARM64 boot mapping mismatch: PC-relative and absolute __kernel_text_start differ"
    );

    // Configure platform addresses while the loader's HardwareConfig remains
    // accessible through TTBR0, before any device or secondary CPU is started.
    if hw_config_ptr != 0 {
        let config = unsafe { &*(hw_config_ptr as *const kernel::platform_config::HardwareConfig) };
        assert!(
            kernel::platform_config::init_from_parallels(config),
            "ARM64 boot: incompatible loader HardwareConfig"
        );

        // Breadcrumb: 'H' = linked HHDM entry and platform setup complete.
        unsafe {
            let uart_phys = kernel::platform_config::uart_base_phys();
            let uart_hhdm = (0xFFFF_0000_0000_0000u64 + uart_phys) as *mut u8;
            core::ptr::write_volatile(uart_hhdm, b'H');
        }
    }

    // Start the initialization watchdog after any required high-half transition
    // but before kernel initialization or SMP release. This watchdog bounds
    // pre-test initialization (secondary-CPU bring-up) ONLY; the boot-test phase
    // has its own separately anchored budget, so a slow initialization can no
    // longer consume a later gate's promised window.
    // CNTVCT is architectural and readable before timer calibration.
    #[cfg(feature = "boot_tests")]
    kernel::test_framework::begin_initialization_watchdog(timer::rdtsc_serialized());

    // Zero the .dma section (Non-Cacheable DMA buffer region).
    // This memory is NOLOAD in the ELF, so neither the loader nor boot.S zeroes it.
    // Must happen before any driver init that uses DMA buffers.
    unsafe {
        extern "C" {
            static __dma_start: u8;
            static __dma_end: u8;
        }
        let start = &__dma_start as *const u8 as *mut u8;
        let end = &__dma_end as *const u8;
        let len = end as usize - start as usize;
        if len > 0 && len < 0x20_0000 {
            core::ptr::write_bytes(start, 0, len);
        }
    }

    // Install the kernel's exception vector table (VBAR_EL1).
    // On QEMU, boot.S already did this before jumping to kernel_main.
    // On Parallels, the UEFI loader installed minimal "write X and spin" vectors.
    // We must install the real kernel vectors before any interrupt fires.
    // NOTE: After the HHDM switch above, exception_vectors resolves to an HHDM address.
    unsafe {
        extern "C" {
            fn exception_vectors();
        }
        let vectors_addr = exception_vectors as usize as u64;
        core::arch::asm!(
            "msr vbar_el1, {v}",
            "isb",
            v = in(reg) vectors_addr,
            options(nostack, preserves_flags),
        );
    }

    // Breadcrumb: 'E' = exception vectors installed
    kernel::serial_line::Line::new().char(b'E');

    // Initialize physical memory offset (needed for MMIO access)
    kernel::memory::init_physical_memory_offset_aarch64();

    // Breadcrumb: 'O' = physical memory offset initialized
    kernel::serial_line::Line::new().char(b'O');

    // Initialize serial output first so we can print
    serial::init_serial();

    // Breadcrumb: 'S' = serial initialized
    kernel::serial_line::Line::new().char(b'S');

    // Install debug-only sentinels between the scheduler and idle/exception
    // halves before either scheduler startup or secondary-CPU bring-up.
    kernel::arch_impl::aarch64::constants::initialize_percpu_stack_boundary_canaries();

    // Initialize the /proc/kmsg log buffer early so ALL serial output is captured
    kernel::log_buffer::init();

    serial_println!();
    serial_println!("========================================");
    serial_println!("  Breenix ARM64 Kernel Starting");
    serial_println!("  BUILD_ID: {}", env!("BREENIX_BUILD_ID"));
    serial_println!("========================================");
    serial_println!();
    boot_screen::stage(Stage::KernelStarting);

    // #596 anti-vacuity: name the inline-save resume-point oracle and whether
    // the forced-ERET repro knob is compiled in. Emitted after serial init, so
    // a boot that reaches serial at all is scored against the oracle.
    #[cfg(feature = "boot_tests")]
    serial_println!(
        "[CTX596_ORACLE:ARMED:force_eret={}]",
        if cfg!(feature = "force_eret_dispatch_596") {
            1
        } else {
            0
        }
    );

    // Diagnostic: verify this code is reached (no format args = no alloc issues)
    serial_println!("[boot] DIAG_MARKER_XHCI_A");
    let hcrst_raw = kernel::platform_config::xhci_hcrst_done_raw();
    serial_println!("[boot] DIAG_MARKER_XHCI_B");
    let ecam = kernel::platform_config::pci_ecam_base();
    serial_println!("[boot] DIAG_MARKER_XHCI_C");
    serial_println!(
        "[boot] loader xhci_hcrst_raw=0x{:x} ecam=0x{:x}",
        hcrst_raw,
        ecam
    );

    // Print CPU info
    let el = current_exception_level();
    serial_println!("[boot] Current exception level: EL{}", el);
    if el == 1 {
        boot_screen::stage(Stage::RunningAtEl1);
    }

    serial_println!("[boot] MMU already enabled (high-half kernel)");
    boot_screen::stage(Stage::MmuEnabled);

    // Zero the boot identity map L0 entry to prevent new TLB entries from
    // being created for the user VA range while we're still in kernel init.
    // This is a defense-in-depth measure; the TLBI in launch_init_from_elf
    // will do a full invalidation before switching to the process page table.
    //
    // We can't use `extern "C" { static mut ttbr0_l0: u64; }` because the
    // symbol is in .bss.boot (low physical memory) while the kernel runs in
    // the high half -- the ADRP relocation would be out of range (~281 TB).
    // Instead, read the current TTBR0_EL1 to get the physical address and
    // access it through the HHDM.
    // Initialize memory management for ARM64
    // ARM64 QEMU virt machine: RAM starts at 0x40000000
    // Frame allocator range from platform_config (QEMU defaults or HardwareConfig)
    let fa_start = kernel::platform_config::frame_alloc_start();
    let fa_end = kernel::platform_config::frame_alloc_end();
    serial_println!(
        "[boot] Initializing memory management ({:#x}-{:#x})...",
        fa_start,
        fa_end
    );
    kernel::memory::frame_allocator::init_aarch64(fa_start, fa_end);
    kernel::memory::init_aarch64_heap();
    kernel::memory::frame_allocator::init_frame_ledger();
    kernel::memory::kernel_stack::init();
    serial_println!("[boot] Memory management ready");
    boot_screen::stage(Stage::MemoryReady);

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
    serial_println!(
        "[boot] Timer frequency: {} Hz ({} MHz)",
        freq,
        freq / 1_000_000
    );
    boot_screen::stage(Stage::TimerCalibrated);

    // Initialize RTC for wall-clock time (PL031 on QEMU virt)
    serial_println!("[boot] Initializing PL031 RTC...");
    kernel::time::rtc::init();

    // Read current timestamp
    let ts = timer::rdtsc();
    serial_println!("[boot] Current timestamp: {}", ts);
    boot_screen::stage(Stage::RtcRead);

    // Initialize GIC
    serial_println!("[boot] Initializing GIC...");
    Gicv2::init();
    serial_println!("[boot] GIC initialized (version {})", gic::active_version());
    boot_screen::stage(Stage::GicInitialized);
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
    boot_screen::stage(Stage::UartInterrupts);

    // Enable interrupts
    serial_println!("[boot] Enabling interrupts...");
    unsafe {
        Aarch64Cpu::enable_interrupts();
    }
    let irq_enabled = Aarch64Cpu::interrupts_enabled();
    serial_println!("[boot] Interrupts enabled: {}", irq_enabled);
    boot_screen::stage(Stage::InterruptsEnabled);

    // Read display resolution from fw_cfg before driver init (QEMU only;
    // fw_cfg device at 0x09020000 doesn't exist on Parallels)
    if kernel::platform_config::is_qemu() {
        kernel::drivers::virtio::gpu_mmio::load_resolution_from_fw_cfg();
    }

    // On QEMU, enable PCI ECAM so AHCI controllers can be discovered.
    // QEMU's virt machine always has a PCI host bridge at ECAM 0x40_1000_0000;
    // this is a no-op if no PCI devices are attached (scan returns quickly).
    if kernel::platform_config::is_qemu() {
        kernel::platform_config::init_qemu_pci();
        serial_println!(
            "[boot] QEMU PCI ECAM configured at {:#x}",
            kernel::platform_config::pci_ecam_base()
        );
    }

    // Initialize device drivers (VirtIO MMIO enumeration)
    serial_println!("[boot] Initializing device drivers...");
    let device_count = kernel::drivers::init();
    serial_println!("[boot] Found {} devices", device_count);
    boot_screen::stage(Stage::DriversInitialized);
    kernel::drivers::run_post_init_self_tests();
    boot_screen::stage(Stage::DriverSelfTests);
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
            boot_screen::stage(Stage::Ext2Mounted);
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


    // ext2/VFS fault-injection leg (test profile only, feature `fs_fault_inject`).
    // Runs immediately after the root filesystem mounts, so every marker the rest
    // of this boot prints is evidence the kernel survived the injected faults.
    #[cfg(feature = "fs_fault_inject")]
    kernel::fs::fault_inject::run_fs_fault_leg();
    // Initialize ext2 home filesystem (/home on separate disk)
    match kernel::fs::ext2::init_home_fs() {
        Ok(()) => serial_println!("[boot] ext2 home filesystem mounted at /home"),
        Err(e) => serial_println!("[boot] No home filesystem: {} (continuing)", e),
    }

    // Initialize devfs (/dev virtual filesystem)
    kernel::fs::devfs::init();
    serial_println!("[boot] devfs initialized at /dev");
    boot_screen::stage(Stage::Devfs);

    // Initialize devptsfs (/dev/pts pseudo-terminal slave filesystem)
    kernel::fs::devptsfs::init();
    serial_println!("[boot] devptsfs initialized at /dev/pts");
    boot_screen::stage(Stage::Devpts);

    // Detect CPU features (must be before procfs so /proc/cpuinfo has real data)
    kernel::arch_impl::aarch64::cpuinfo::init();
    serial_println!(
        "[boot] CPU detected: {} {}",
        kernel::arch_impl::aarch64::cpuinfo::get()
            .map(|c| c.implementer_name())
            .unwrap_or("Unknown"),
        kernel::arch_impl::aarch64::cpuinfo::get()
            .map(|c| c.part_name())
            .unwrap_or("Unknown")
    );

    // Initialize procfs (/proc virtual filesystem)
    kernel::fs::procfs::init();
    serial_println!("[boot] procfs initialized at /proc");
    boot_screen::stage(Stage::Procfs);
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::PROCFS_INIT);

    // Initialize TTY subsystem (console + PTY infrastructure)
    kernel::tty::init();
    serial_println!("[boot] TTY subsystem initialized");
    boot_screen::stage(Stage::Tty);

    // Initialize graphics based on available hardware (capability-based detection)
    //
    // Graphics initialization priority:
    //   1. VirtIO GPU PCI + GOP hybrid (Parallels): GPU set_scanout configures
    //      the display's reading stride/resolution (e.g. 1280x800), while pixel
    //      data is written to GOP memory (0x10000000). VirtIO transfer_to_host
    //      does NOT update the display — only GOP memory is scanned out. The
    //      kernel MUST write at the GPU-configured stride, not the GOP stride.
    //   2. UEFI GOP framebuffer (fallback if GPU PCI not available)
    //   3. VirtIO GPU MMIO (QEMU virt platform)
    serial_println!("[boot] Initializing graphics...");
    // Graphics initialization:
    //   - The 2D resource + SET_SCANOUT was already done in gpu_pci::init()
    //     (matching Linux: console framebuffer is active before VirGL takes over)
    //   - When VirGL is active, virgl_init() already switched SET_SCANOUT to the
    //     3D resource. Do NOT initialize GOP framebuffer (arm64_fb) because its
    //     flush calls would send RESOURCE_FLUSH on the 2D resource, overriding
    //     the VirGL scanout. The kernel's boot and diagnostics screens instead
    //     draw into the 3D resource's guest backing (gpu_pci::framebuffer) and
    //     present it with TRANSFER_TO_HOST_3D, until userspace takes the display.
    let has_display = if kernel::drivers::virtio::gpu_pci::is_initialized()
        && kernel::drivers::virtio::gpu_pci::is_virgl_enabled()
    {
        serial_println!("[boot] VirGL active — skipping GOP framebuffer init (2D resource already set up in gpu_pci::init)");
        // Populate FB_INFO_CACHE so fbinfo syscall works for userspace programs.
        // VirGL owns the display but userspace still needs dimensions for layout.
        if let Some((w, h)) = kernel::drivers::virtio::gpu_pci::dimensions() {
            let _ = arm64_fb::FB_INFO_CACHE.try_init_once(|| arm64_fb::FbInfoCache {
                width: w as usize,
                height: h as usize,
                stride: w as usize,
                bytes_per_pixel: 4,
                is_bgr: true, // B8G8R8X8_UNORM
            });
            serial_println!(
                "[boot] FB_INFO_CACHE populated for VirGL display ({}x{})",
                w,
                h
            );
        }
        if let Err(e) = arm64_fb::init_shell_framebuffer() {
            serial_println!("[boot] VirGL shell framebuffer failed: {}", e);
        }
        true
    } else if kernel::drivers::virtio::gpu_pci::is_initialized()
        && kernel::platform_config::has_framebuffer()
    {
        // 2D mode: VirtIO GPU PCI set_scanout changes the display's
        // reading stride to the GPU-configured width. Pixels are read
        // from GOP memory at this new stride.
        match arm64_fb::init_gpu_pci_gop_framebuffer() {
            Ok(()) => {
                serial_println!("[boot] GPU PCI+GOP hybrid display initialized");
                if let Err(e) = init_gop_display() {
                    serial_println!("[boot] Display setup failed: {}", e);
                }
                true
            }
            Err(e) => {
                serial_println!("[boot] GPU PCI+GOP hybrid failed: {}, trying pure GOP", e);
                match arm64_fb::init_gop_framebuffer() {
                    Ok(()) => {
                        serial_println!("[boot] GOP framebuffer initialized (fallback)");
                        if let Err(e) = init_gop_display() {
                            serial_println!("[boot] GOP display setup failed: {}", e);
                        }
                        true
                    }
                    Err(e2) => {
                        serial_println!("[boot] GOP framebuffer also failed: {}", e2);
                        false
                    }
                }
            }
        }
    } else if kernel::platform_config::has_framebuffer() {
        // UEFI GOP framebuffer (fallback when GPU PCI not available)
        match arm64_fb::init_gop_framebuffer() {
            Ok(()) => {
                serial_println!("[boot] GOP framebuffer initialized");
                // Draw initial split-screen layout
                if let Err(e) = init_gop_display() {
                    serial_println!("[boot] GOP display setup failed: {}", e);
                }
                true
            }
            Err(e) => {
                serial_println!("[boot] GOP framebuffer failed: {}", e);
                false
            }
        }
    } else if kernel::platform_config::is_qemu() {
        // VirtIO GPU MMIO (QEMU virt platform)
        match init_graphics() {
            Ok(()) => true,
            Err(e) => {
                serial_println!("[boot] VirtIO graphics failed: {}", e);
                false
            }
        }
    } else {
        serial_println!("[boot] No display device found");
        false
    };

    // Track whether VirGL owns the display (Phase 1: textured quad test only)
    let virgl_display = kernel::drivers::virtio::gpu_pci::is_initialized()
        && kernel::drivers::virtio::gpu_pci::is_virgl_enabled();

    // Upgrade framebuffer to double buffering now that heap is available.
    // This allocates a shadow buffer in cached RAM so pixel writes are fast
    // (~1ns vs ~100ns for direct GOP BAR0 writes on Parallels).
    // With VirGL the shell framebuffer is the 3D scanout's backing; the boot
    // screen presents it synchronously and stops once userspace takes the display.
    if has_display && arm64_fb::SHELL_FRAMEBUFFER.get().is_some() {
        kernel::graphics::arm64_fb::upgrade_to_double_buffer();
        // The screen shows the boot screen; kernel log lines stay on serial unless
        // -fw_cfg name=opt/breenix/fbconsole,string=log asks for them on screen.
        if read_fbconsole_log() {
            kernel::graphics::log_console::start();
        } else {
            boot_screen::set_last_stage_label(if cfg!(feature = "testing") {
                "Starting test programs"
            } else {
                "Starting PID 1"
            });
            boot_screen::show();
        }
    }

    // Initialize input devices (capability-based detection)
    if kernel::drivers::usb::xhci::is_initialized() {
        // USB HID keyboard/mouse via XHCI — already set up during drivers::init().
        // Runtime input and command completion delivery are IRQ-driven.
        serial_println!("[boot] USB HID input active via XHCI IRQ");
    } else if kernel::platform_config::is_qemu() {
        // VirtIO keyboard/mouse MMIO (QEMU)
        serial_println!("[boot] Initializing VirtIO keyboard...");
        match input_mmio::init() {
            Ok(()) => serial_println!("[boot] VirtIO keyboard initialized"),
            Err(e) => serial_println!("[boot] VirtIO keyboard init failed: {}", e),
        }
    }

    // TLB eviction for TTBR0 identity map is handled in launch_init_from_elf()
    // right before the TTBR0 switch. We cannot modify TTBR0 page tables earlier
    // because Parallels monitors TTBR0/page table changes and hangs if they occur
    // while timer interrupts and kthreads are active.

    // Initialize per-CPU data (required before scheduler and interrupts)
    serial_println!("[boot] Initializing per-CPU data...");
    kernel::per_cpu_aarch64::init();
    // Store the boot TTBR0 as the kernel page table for this CPU.
    // Without this, exception handlers fall back to a wrong hardcoded address.
    let boot_ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) boot_ttbr0, options(nomem, nostack));
    }
    kernel::per_cpu_aarch64::set_kernel_cr3(boot_ttbr0);
    kernel::arch_impl::aarch64::cache::grant_el0_cache_maintenance();
    serial_println!("[boot] Per-CPU data initialized");
    boot_screen::stage(Stage::PerCpu);

    // Disable preemption for the boot sequence on CPU 0.
    // The timer interrupt fires while kernel_main is running and the scheduler
    // may try to context-switch the boot CPU to ksoftirqd. When ksoftirqd parks,
    // setup_idle_return_locked redirects CPU 0 to idle_loop_arm64, abandoning
    // the boot sequence. Preventing preemption here keeps the boot CPU on its
    // boot stack until init is ready to run.
    // Re-enabled in launch_init_from_elf before spawn_as_current.
    kernel::per_cpu_aarch64::preempt_disable();

    // Initialize process manager
    serial_println!("[boot] Initializing process manager...");
    kernel::process::init();
    serial_println!("[boot] Process manager initialized");
    boot_screen::stage(Stage::ProcessManager);

    // Initialize scheduler with an idle task
    serial_println!("[boot] Initializing scheduler...");
    init_scheduler();
    serial_println!("[boot] Scheduler initialized");
    boot_screen::stage(Stage::Scheduler);
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::SCHEDULER_INIT);

    // Initialize workqueue subsystem (depends on kthread infrastructure)
    kernel::task::workqueue::init_workqueue();
    kernel::fs::ext2::writeback::init().expect("ext2 finalization service");
    serial_println!("[boot] Workqueue subsystem initialized");
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::WORKQUEUE_INIT);

    // Initialize softirq subsystem (depends on kthread infrastructure)
    kernel::task::softirqd::init_softirq();
    kernel::net::init_loopback_pump();
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
    // Spawn render thread if any display is available (GOP or VirtIO)
    // Skip when VirGL owns the display — no 2D rendering needed.
    if has_display && !virgl_display {
        match kernel::graphics::render_task::spawn_render_thread() {
            Ok(tid) => serial_println!("[boot] Render thread spawned (tid={})", tid),
            Err(e) => serial_println!("[boot] Failed to spawn render thread: {}", e),
        }
    } else if virgl_display {
        serial_println!("[boot] Render thread skipped — VirGL owns the display");
        // Without a render thread, a kernel screen whose synchronous present
        // failed would stay off the display; this thread presents it later.
        if arm64_fb::SHELL_FRAMEBUFFER.get().is_some() {
            match kernel::graphics::boot_screen::spawn_presenter() {
                Ok(tid) => serial_println!("[boot] Screen presenter spawned (tid={})", tid),
                Err(e) => serial_println!("[boot] Failed to spawn screen presenter: {}", e),
            }
        }
    }

    // Initialize tracing subsystem (must be after allocator, before timer)
    kernel::tracing::init();
    kernel::tracing::providers::init();
    kernel::tracing::enable();
    kernel::tracing::providers::enable_all();
    serial_println!("[boot] Tracing subsystem initialized and enabled");

    // Pre-load init binary from ext2 BEFORE timer is initialized.
    // AHCI polling uses `wfe` to yield between PORT_CI checks. With the timer
    // active, `wfe` wakes every 1ms on timer ticks instead of AHCI completion,
    // and on 8GB RAM something about the timer GIC activity prevents PORT_CI
    // from clearing. Reading the ELF now, while the timer is still off and
    // single-CPU, avoids both issues entirely.
    // (ext2 root filesystem was mounted above at init_root_fs().)
    // Probe, program and suite modes pre-load their own binary (/sbin/probe, or
    // /sbin/suite-<id>) the same way and fall back to /sbin/init.
    let (mut boot_mode, mut mode_arg) = read_boot_mode();
    let mut init_launch = InitLaunch::for_mode(boot_mode, mode_arg.as_deref());
    let runs_own_binary = matches!(
        boot_mode,
        BootMode::Probe | BootMode::Program | BootMode::Suite
    );
    let mode_elf: Option<alloc::vec::Vec<u8>> = if runs_own_binary && device_count > 0 {
        serial_println!(
            "[boot] Pre-loading {} from ext2 (before timer)...",
            init_launch.path
        );
        match read_init_from_ext2(&init_launch.path) {
            Ok(data) => match kernel::boot::target::check_elf(&data) {
                Ok(()) => {
                    serial_println!(
                        "[boot] {} pre-loaded: {} bytes",
                        init_launch.path,
                        data.len()
                    );
                    Some(data)
                }
                Err(e) => {
                    serial_println!("[boot] {} is not a loadable ELF: {}", init_launch.path, e);
                    None
                }
            },
            Err("init not found") => {
                serial_println!("[boot] {} not found", init_launch.path);
                None
            }
            Err(e) => {
                serial_println!("[boot] Failed to pre-load {}: {}", init_launch.path, e);
                None
            }
        }
    } else {
        None
    };
    if runs_own_binary && mode_elf.is_none() {
        serial_println!(
            "[boot] Ignoring boot mode {:?}: {} could not be loaded",
            boot_mode.name(),
            init_launch.path
        );
        boot_mode = BootMode::Default;
        mode_arg = None;
        init_launch = InitLaunch::default_init();
    }
    announce_boot_mode(boot_mode, mode_arg.as_deref());
    let init_elf: Option<alloc::vec::Vec<u8>> = if mode_elf.is_some() {
        mode_elf
    } else if device_count > 0 {
        serial_println!("[boot] Pre-loading /sbin/init from ext2 (before timer)...");
        match read_init_from_ext2("/sbin/init") {
            Ok(data) => {
                serial_println!("[boot] Init binary pre-loaded: {} bytes", data.len());
                Some(data)
            }
            Err(e) => {
                serial_println!(
                    "[boot] Failed to pre-load init: {} (will retry after SMP)",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    kernel::drivers::usb::xhci::activate_msi_if_ready();
    // Boot-context observability for Parallels MSI delivery. This runs once,
    // before timer init, so it does not depend on CPU0 timer polling.
    serial_println!(
        "[xhci] post-activation: MSI_EVENT_COUNT={} EVENT_COUNT={} POLL_COUNT={} SPI_ACTIVATED={}",
        kernel::drivers::usb::xhci::MSI_EVENT_COUNT.load(core::sync::atomic::Ordering::Relaxed),
        kernel::drivers::usb::xhci::EVENT_COUNT.load(core::sync::atomic::Ordering::Relaxed),
        kernel::drivers::usb::xhci::POLL_COUNT.load(core::sync::atomic::Ordering::Relaxed),
        kernel::drivers::usb::xhci::SPI_ACTIVATED.load(core::sync::atomic::Ordering::Relaxed),
    );

    // Initialize timer interrupt for preemptive scheduling
    // This MUST come after per-CPU data and scheduler are initialized
    serial_println!("[boot] Initializing timer interrupt...");
    timer_interrupt::init();
    serial_println!("[boot] Timer interrupt initialized");
    boot_screen::stage(Stage::TimerInterrupt);
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::pass(kernel::test_framework::catalog::AARCH64_TIMER_INIT);

    // What the firmware reports, read before any secondary is started.
    let (reported_cpus, reported_cpus_source) = kernel::arch_impl::aarch64::smp::reported_cpus();

    // Bring up secondary CPUs via PSCI CPU_ON.
    // Probe-based: try each CPU ID and let PSCI tell us which exist.
    //
    // Parallels is included again for F29 validation. F21's secondary CPU fault
    // may have been fixed by later GICR/AHCI/timer changes; if it still
    // reproduces, this branch must document that failure rather than merging.
    if kernel::arch_impl::aarch64::gic::use_group0()
        && kernel::arch_impl::aarch64::gic::ds_enabled()
    {
        serial_println!("[smp] Refusing secondary CPU probe: dual-group GIC acknowledgement requires single-CPU operation");
    } else if kernel::platform_config::is_qemu()
        || kernel::platform_config::is_vmware()
        || kernel::platform_config::is_parallels()
    {
        // Write per-CPU stack base address. Placed AFTER kernel image + full BSS
        // (which includes large statics like PCI_3D_FRAMEBUFFER extending to ~0x43000000).
        // Platform-dependent:
        // QEMU/Parallels (ram at 0x40000000): 0x4300_0000
        // VMware (ram at 0x80000000): 0x8300_0000
        let stack_base_phys = 0x4300_0000u64 + kernel::platform_config::ram_base_offset();
        kernel::arch_impl::aarch64::smp::set_stack_base_phys(stack_base_phys);

        // Write CPU 0's actual TTBR0/TTBR1 to .bss.boot so secondary CPUs use
        // the correct page tables. On Parallels, the UEFI loader builds its own
        // page tables (not boot.S's ttbr0_l0/ttbr1_l0), so we must pass the
        // real TTBR values to secondary CPUs explicitly.
        kernel::arch_impl::aarch64::smp::set_smp_ttbrs();

        // Log CPU 0's MPIDR for topology diagnostics
        let mpidr: u64;
        unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)) };
        serial_println!(
            "[smp] CPU 0 MPIDR={:#x}, stack_base={:#x}",
            mpidr,
            stack_base_phys
        );

        // Derive the maximum number of CPUs to probe from the GICv3 redistributor
        // region size. Each redistributor occupies 0x20000 bytes (two 64KB frames).
        // This prevents us from issuing a PSCI CPU_ON HVC for a CPU index that the
        // hypervisor cannot service — that call blocks forever since HVC is a
        // synchronous trap with no software timeout.
        //
        // On QEMU, gicr_size() returns 0 (no explicit size reported), so we fall
        // back to MAX_CPUS and let PSCI error codes guide the probe.
        // On Parallels with N vCPUs, gicr_size() = N * 0x20000, giving us the
        // exact upper bound.
        const GICR_FRAME_SIZE: u64 = 0x2_0000; // 128KB per CPU (GICv3 spec)
        let gicr_size = kernel::platform_config::gicr_size();
        let max_cpus_to_probe = if gicr_size > 0 {
            let n = (gicr_size / GICR_FRAME_SIZE) as usize;
            let capped = n.min(kernel::arch_impl::aarch64::smp::MAX_CPUS);
            serial_println!(
                "[smp] GICR covers {} redistributors, probing CPUs 1..{}",
                n,
                capped
            );
            capped
        } else {
            kernel::arch_impl::aarch64::smp::MAX_CPUS
        };

        serial_println!("[smp] Probing secondary CPUs via PSCI...");
        let mut launched = 0u64;
        for cpu in 1..max_cpus_to_probe {
            let ret = kernel::arch_impl::aarch64::smp::release_cpu(cpu);
            if ret == 0 {
                serial_println!(
                    "[smp] CPU {}: PSCI CPU_ON success (raw_status={})",
                    cpu,
                    kernel::arch_impl::aarch64::smp::last_psci_return_code(cpu)
                );
                launched += 1;
            } else {
                serial_println!(
                    "[smp] CPU {}: PSCI CPU_ON failed (ret={}), stopping probe",
                    cpu,
                    ret
                );
                break;
            }
        }
        if launched > 0 {
            #[cfg(feature = "boot_tests")]
            const SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS: u64 = 10;
            #[cfg(not(feature = "boot_tests"))]
            const SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS: u64 = 2;
            // #522 C5 fix pass (V-1, V-3): this local ceiling was 40s until the
            // review round found the initialization watchdog
            // (test_framework::INITIALIZATION_WATCHDOG_BUDGET_MILLISECONDS)
            // anchored strictly earlier than this wait's own `start`, at equal
            // magnitude, so it always reached its deadline first and this local
            // ceiling's own diagnostic was unreachable in every boot_tests boot
            // -- the opposite of the "not a tighter bound" claim published
            // alongside it. The watchdog's ceiling is now computed dynamically
            // below as max(published floor, this local ceiling + the measured
            // gap between the two anchors + a fixed margin), which guarantees
            // the watchdog cannot reach its deadline before this local ceiling
            // does. That guarantee interacts with #522 C5 review V-3: the
            // pre-fix pair (40s watchdog + 60s test-phase budget) summed to
            // 100s, past the strict harness's 90s hard timeout. Halving this
            // local ceiling (and, in step, the no-progress window above and the
            // watchdog's published floor in mod.rs) keeps the summed pair at
            // 80s -- ten seconds inside the hard timeout -- while remaining
            // 1.5x the realistic worst case and 1.875x the longest individually
            // observed wait documented below.
            // claim-lint:ok: #522 C5 fix pass; the dynamic ceiling formula and
            // the reduced constants are pinned in tests/teardown_structure.rs.
            // The realistic starved proof remains far inside this backstop:
            // <=10s total, with the longest observed individual wait about 8s.
            #[cfg(feature = "boot_tests")]
            const SMP_ONLINE_ABSOLUTE_CEILING_SECONDS: u64 = 15;
            #[cfg(not(feature = "boot_tests"))]
            const SMP_ONLINE_ABSOLUTE_CEILING_SECONDS: u64 = 4;
            // Fixed safety margin added atop the local absolute ceiling when
            // computing the initialization watchdog's dynamic ceiling below, so
            // the watchdog's deadline is strictly later than the local
            // ceiling's own deadline rather than tying with it (a tie would
            // still leave the local ceiling's diagnostic unreachable, because
            // the watchdog is checked first in the loop below).
            #[cfg(feature = "boot_tests")]
            const INITIALIZATION_WATCHDOG_LOCAL_CEILING_MARGIN_SECONDS: u64 = 3;
            const SMP_ONLINE_BREADCRUMB_INTERVAL_SECONDS: u64 = 1;
            const SMP_ONLINE_STAGE_SAMPLE_INTERVAL_ITERATIONS: u64 = 4_096;
            const SMP_ONLINE_CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 10_000_000;

            // Wait for all launched CPUs to come online.
            let expected = 1 + launched; // boot CPU + launched
            let reported_frequency_hz = timer::frequency_hz();
            let counter_frequency_hz = if reported_frequency_hz == 0 {
                // P17/P19 deliberately use a short 1MHz bring-up fallback;
                // the boot-test watchdog P20 instead fails closed because a
                // guessed frequency could silently stretch their harness bound.
                kernel::arch_impl::aarch64::timer::BOOT_COUNTER_FALLBACK_FREQUENCY_HZ
            } else {
                reported_frequency_hz
            };
            let no_progress_ticks =
                counter_frequency_hz.saturating_mul(SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS);
            let absolute_ceiling_ticks =
                counter_frequency_hz.saturating_mul(SMP_ONLINE_ABSOLUTE_CEILING_SECONDS);
            #[cfg(feature = "boot_tests")]
            let initialization_watchdog_started_at =
                kernel::test_framework::initialization_watchdog_started_at();
            let breadcrumb_ticks =
                counter_frequency_hz.saturating_mul(SMP_ONLINE_BREADCRUMB_INTERVAL_SECONDS);
            let stage_at_start: [u32; kernel::arch_impl::aarch64::smp::MAX_CPUS] =
                core::array::from_fn(kernel::arch_impl::aarch64::smp::bringup_stage_of);
            let mut last_online = kernel::arch_impl::aarch64::smp::cpus_online();
            let mut last_bringup_progress = kernel::arch_impl::aarch64::smp::bringup_progress();
            let start = timer::rdtsc();
            // #522 C5 fix pass (V-1): compute the watchdog's real per-boot
            // ceiling here, now that `start` is known, as max(published floor,
            // local ceiling + measured gap + margin). The measured gap is the
            // actual elapsed time between the kernel-entry anchor and this
            // wait's own `start`, sampled fresh via rdtsc rather than assumed
            // negligible, so -- unlike the flat same-magnitude constant it
            // replaces -- this construction cannot reach its deadline before
            // the local absolute ceiling's own deadline does.
            #[cfg(feature = "boot_tests")]
            let initialization_watchdog_ceiling_ticks = {
                let published_floor_ticks = timer::milliseconds_to_ticks(
                    counter_frequency_hz,
                    kernel::test_framework::INITIALIZATION_WATCHDOG_BUDGET_MILLISECONDS,
                );
                match initialization_watchdog_started_at {
                    Some(initialization_started_at) => {
                        let pre_wait_gap_ticks =
                            timer::elapsed_ticks(start, initialization_started_at);
                        let margin_ticks = counter_frequency_hz.saturating_mul(
                            INITIALIZATION_WATCHDOG_LOCAL_CEILING_MARGIN_SECONDS,
                        );
                        let gap_extended_ticks = absolute_ceiling_ticks
                            .saturating_add(pre_wait_gap_ticks)
                            .saturating_add(margin_ticks);
                        let effective_ticks = published_floor_ticks.max(gap_extended_ticks);
                        serial_println!(
                            "[smp] initialization_watchdog gap_ms={} local_ceiling_ms={} margin_ms={} effective_ceiling_ms={}",
                            pre_wait_gap_ticks.saturating_mul(1_000) / counter_frequency_hz.max(1),
                            SMP_ONLINE_ABSOLUTE_CEILING_SECONDS.saturating_mul(1_000),
                            INITIALIZATION_WATCHDOG_LOCAL_CEILING_MARGIN_SECONDS.saturating_mul(1_000),
                            effective_ticks.saturating_mul(1_000) / counter_frequency_hz.max(1),
                        );
                        effective_ticks
                    }
                    None => published_floor_ticks,
                }
            };
            let mut last_advance = start;
            let mut last_breadcrumb = start;
            let mut last_counter_sample = start;
            let mut iterations = 0u64;
            let report_offline_diagnostics = || {
                // The probe stops at its first failure, so launched CPU IDs
                // are contiguous and every possible missing CPU is in this range.
                for cpu in 1..expected as usize {
                    if !kernel::arch_impl::aarch64::smp::is_cpu_online(cpu) {
                        let stage_now = kernel::arch_impl::aarch64::smp::bringup_stage_of(cpu);
                        let last_psci = kernel::arch_impl::aarch64::smp::last_psci_return_code(cpu);
                        serial_println!(
                            "[smp] CPU {} still offline: stage={} {} last PSCI return code {} ({}) stage_at_start={} stage_advanced={}",
                            cpu,
                            stage_now,
                            kernel::arch_impl::aarch64::smp::bringup_stage_name(stage_now),
                            last_psci,
                            kernel::arch_impl::aarch64::smp::psci_return_code_name(last_psci),
                            stage_at_start[cpu],
                            stage_now > stage_at_start[cpu]
                        );
                    }
                }
            };

            // SMP secondary CPU bring-up wait: boot CPU spins until all
            // released secondary CPUs increment cpus_online. The boot_tests
            // build retains the starvation-tolerant 10s/15s bounds (halved
            // from 20s/40s by #522 C5 fix pass V-3, still comfortably above
            // the realistic worst case documented below); the plain kernel
            // uses 2s/4s. The no-progress window is re-armed only when
            // cpus_online or the sum of secondary bring-up stages advances. In
            // boot_tests, the initialization watchdog's ceiling is computed
            // dynamically above from this local ceiling itself, adding the
            // measured gap plus margin, so it can only be reached AT OR AFTER
            // this local ceiling's own deadline, never before it -- #522 C5
            // fix pass V-1 replaced an earlier same-magnitude constant that
            // could (and, given real pre-wait initialization time, always did)
            // fire first.
            // claim-lint:ok: #522 C5 fix pass; the dynamic formula and the two
            // budgets are pinned in tests/teardown_structure.rs. With a
            // running CNTVCT those ceilings bound the loop; if CNTVCT freezes,
            // the unconditional delta sample bounds it by iterations instead. No
            // IRQ can signal "CPU online" before each CPU wires its GIC, so this
            // bounded CPU-management handshake is allowlisted per
            // docs/polling-allowlist.md.
            loop {
                let now = timer::rdtsc();
                let current_online = kernel::arch_impl::aarch64::smp::cpus_online();
                if current_online >= expected {
                    break;
                }
                if current_online > last_online {
                    last_online = current_online;
                    last_advance = now;
                } else if iterations % SMP_ONLINE_STAGE_SAMPLE_INTERVAL_ITERATIONS == 0 {
                    let current_bringup_progress =
                        kernel::arch_impl::aarch64::smp::bringup_progress();
                    if current_bringup_progress > last_bringup_progress {
                        last_bringup_progress = current_bringup_progress;
                        last_advance = now;
                    }
                }

                #[cfg(feature = "boot_tests")]
                if let Some(initialization_started_at) = initialization_watchdog_started_at {
                    if timer::elapsed_ticks(now, initialization_started_at)
                        >= initialization_watchdog_ceiling_ticks
                    {
                        let online_at_verdict = kernel::arch_impl::aarch64::smp::cpus_online();
                        if online_at_verdict >= expected {
                            break;
                        }
                        serial_println!(
                            "[smp] Timeout waiting for CPUs: initialization watchdog ceiling of {} ms reached ({} online, {} expected)",
                            initialization_watchdog_ceiling_ticks.saturating_mul(1_000)
                                / counter_frequency_hz.max(1),
                            online_at_verdict,
                            expected
                        );
                        report_offline_diagnostics();
                        break;
                    }
                } else {
                    let online_at_verdict = kernel::arch_impl::aarch64::smp::cpus_online();
                    if online_at_verdict >= expected {
                        break;
                    }
                    serial_println!(
                        "[smp] initialization watchdog anchor unavailable ({} online, {} expected)",
                        online_at_verdict,
                        expected
                    );
                    report_offline_diagnostics();
                    break;
                }

                if timer::elapsed_ticks(now, start) >= absolute_ceiling_ticks {
                    let online_at_verdict = kernel::arch_impl::aarch64::smp::cpus_online();
                    if online_at_verdict >= expected {
                        break;
                    }
                    serial_println!(
                        "[smp] Timeout waiting for CPUs: absolute ceiling of {} seconds reached ({} online, {} expected)",
                        SMP_ONLINE_ABSOLUTE_CEILING_SECONDS,
                        online_at_verdict,
                        expected
                    );
                    report_offline_diagnostics();
                    break;
                }

                if timer::elapsed_ticks(now, last_advance) >= no_progress_ticks {
                    let online_at_verdict = kernel::arch_impl::aarch64::smp::cpus_online();
                    if online_at_verdict >= expected {
                        break;
                    }
                    let progress_at_verdict = kernel::arch_impl::aarch64::smp::bringup_progress();
                    if online_at_verdict > last_online
                        || progress_at_verdict > last_bringup_progress
                    {
                        last_online = online_at_verdict;
                        last_bringup_progress = progress_at_verdict;
                        last_advance = timer::rdtsc();
                        continue;
                    }
                    serial_println!(
                        "[smp] Timeout waiting for CPUs: no progress for {} seconds ({} online, {} expected)",
                        SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS,
                        online_at_verdict,
                        expected
                    );
                    report_offline_diagnostics();
                    break;
                }

                iterations = iterations.wrapping_add(1);
                if iterations % SMP_ONLINE_CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS == 0 {
                    let counter_delta = timer::elapsed_ticks(now, last_counter_sample);
                    if counter_delta == 0 {
                        let online_at_verdict = kernel::arch_impl::aarch64::smp::cpus_online();
                        if online_at_verdict >= expected {
                            break;
                        }
                        serial_println!(
                            "[smp] CNTVCT stalled while waiting for CPUs ({} online, {} expected)",
                            online_at_verdict,
                            expected
                        );
                        report_offline_diagnostics();
                        break;
                    }
                    last_counter_sample = now;
                }

                if timer::elapsed_ticks(now, last_breadcrumb) >= breadcrumb_ticks {
                    for cpu in 1..expected as usize {
                        if !kernel::arch_impl::aarch64::smp::is_cpu_online(cpu) {
                            let stage_now = kernel::arch_impl::aarch64::smp::bringup_stage_of(cpu);
                            serial_println!(
                                "[smp] still waiting, {} online (cpu{} stage={} {})",
                                current_online,
                                cpu,
                                stage_now,
                                kernel::arch_impl::aarch64::smp::bringup_stage_name(stage_now)
                            );
                        }
                    }
                    last_breadcrumb = now;
                }
                core::hint::spin_loop();
            }
        }
        serial_println!(
            "[smp] {} CPUs online",
            kernel::arch_impl::aarch64::smp::cpus_online()
        );
        boot_screen::stage(Stage::SmpOnline);
        kernel::arch_impl::aarch64::gic::init_gicr_rdist_map(
            kernel::arch_impl::aarch64::smp::cpus_online() as usize,
        );
    }

    // The SMP measurement: how many CPUs the firmware reported and how many
    // came online. The second line is the boot-path stage, printed only when
    // there is more than one CPU and every one of them is online.
    let online_cpus = kernel::arch_impl::aarch64::smp::cpus_online();
    serial_println!(
        "[smp] online={} reported={} source={}",
        online_cpus,
        reported_cpus,
        reported_cpus_source
    );
    if reported_cpus > 1 && online_cpus == reported_cpus {
        serial_println!(
            "[smp] every reported CPU is online ({} of {})",
            online_cpus,
            reported_cpus
        );
    }

    kernel::task::softirqd::init_online_daemons();
    #[cfg(all(feature = "boot_tests", not(feature = "testing")))]
    kernel::task::softirq_tests::test_deferral();

    // Failure-capture PR-7's `edge=LOCKUP` oracle (test profile only, feature
    // `capture_lockup_oracle`). Placed here on purpose: SMP bring-up above has
    // finished, so a CPU1-pinned holder can actually be dispatched, and no
    // boot-test coordinator or userspace process has started yet, so no other code
    // is making the context-switch/syscall progress the soft-lockup
    // detector treats as liveness. It returns.
    #[cfg(all(target_arch = "aarch64", feature = "capture_lockup_oracle"))]
    kernel::capture_lockup_oracle::run_lockup_capture_oracle();

    // The kthread and workqueue self-tests below return with interrupts
    // disabled, the state x86 calls them in. This boot thread already runs its
    // timer and the secondary CPUs, so it takes its own state back afterwards:
    // left masked, CPU0 stops taking timer ticks and the boot tests that follow
    // fail on it (#1120).
    #[cfg(feature = "testing")]
    let boot_thread_irqs = kernel::arch_interrupts_enabled();

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
    #[cfg(feature = "testing")]
    if boot_thread_irqs {
        unsafe { kernel::arch_enable_interrupts() };
    }

    // In kthread_test_only mode, exit immediately after kthread tests pass
    #[cfg(feature = "kthread_test_only")]
    {
        serial_println!("=== KTHREAD_TEST_ONLY: All kthread tests passed ===");
        serial_println!("KTHREAD_TEST_ONLY_COMPLETE");
        kernel::exit_qemu(kernel::QemuExitCode::Success);
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    // In workqueue_test_only mode, exit immediately after workqueue test
    #[cfg(feature = "workqueue_test_only")]
    {
        serial_println!("=== WORKQUEUE_TEST_ONLY: All workqueue tests passed ===");
        serial_println!("WORKQUEUE_TEST_ONLY_COMPLETE");
        kernel::exit_qemu(kernel::QemuExitCode::Success);
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
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
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    #[cfg(feature = "testing")]
    kernel::task::userspace_completion::begin_loading();

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

    // #728 ext2 lock-discipline repro oracle (test profile only, feature
    // `ext2_lock_race`). Needs a running scheduler/timer/SMP, so it runs here
    // rather than at the fault-injection leg's early insertion point right
    // after mount -- kthreads do not exist, and SMP CPUs are not yet online,
    // that early in boot.
    #[cfg(feature = "ext2_lock_race")]
    kernel::fs::ext2_lock_race::run_ext2_lock_race_leg();

    kernel::tracing::providers::teardown::emit_root_custody_summary();
    kernel::tracing::providers::teardown::emit_tombstone_census();
    kernel::arch_impl::aarch64::ttbr0::emit_asid_census();
    // The pinned-placement census both aarch64 boot gates fail on. Emitted here
    // because this point is reached in every profile: the production profile
    // has no boot-test sampling kthread, so without this line the production
    // gate would have nothing to assert on. The one-shot marker in
    // `hold_pinned_wake_for_home` is what covers a refusal that happens after
    // this point.
    // claim-lint:ok: 4 of 4 census lines in a strict boot and 1 of 1 in a
    // production boot --
    // docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-boot1-serial.txt
    // and 02-prod-boot1-serial.txt
    // Slice 3e's oracle: three of the eleven migration sites, driven against a
    // thread that carries a per-CPU worker pin, reporting the CPU each one put
    // it on against the CPU the pin names. Boot-tests only, and it runs before
    // the census above so the census reports any refusal the probe's own
    // migrations incidentally cause for a thread other than the probe --
    // `count_pinned_migration_refusal` routes a refusal of the probe's own tid
    // to `PIN_GUARD_ORACLE_REFUSED` instead, so it never reaches the counter
    // this census reads, which is why they must be in this order to be
    // readable together.
    #[cfg(feature = "boot_tests")]
    kernel::task::scheduler::emit_pin_guard_oracle();
    kernel::task::scheduler::emit_pinned_placement_census();

    // #766's wake-to-dispatch latency leg, aarch64 arm. Same leg, same marker
    // shape, so the two architectures can be read against each other; this arm
    // is a regression guard rather than evidence about the x86 mechanism,
    // because aarch64 has MAX_CPUS ready queues and a 1 ms tick. Placed after
    // the pinned-placement census so the 9 threads it creates and joins cannot
    // move what that census reports.
    #[cfg(feature = "boot_tests")]
    kernel::task::timer_wake_oracle::run();

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
    boot_screen::stage(Stage::BootComplete);

    // Spawn particle animation thread (if graphics is available and not running boot tests)
    // This MUST be done BEFORE userspace loading because launch_init_from_elf never returns
    // DISABLED: Investigating EC=0x0 crash during fill_rect memcpy
    #[cfg(not(feature = "boot_tests"))]
    #[cfg(feature = "particle_animation")] // Disabled by default - crashes with EC=0x0
    {
        let has_graphics = kernel::graphics::arm64_fb::SHELL_FRAMEBUFFER
            .get()
            .is_some();
        if has_graphics {
            serial_println!("[graphics] Starting particle animation...");
            match kernel::task::spawn::spawn_thread("particles", particles::animation_thread_entry)
            {
                Ok(tid) => serial_println!("[graphics] Particle animation started (tid={})", tid),
                Err(e) => serial_println!("[graphics] Failed to start animation: {}", e),
            }
        }
    }

    // In testing mode, load test binaries from ext2 and let the scheduler
    // dispatch them. Do NOT call launch_init_from_elf() - its manual
    // spawn_as_current() + return_to_userspace() bypasses the scheduler and
    // conflicts with the 60+ test processes already in the ready queue.
    //
    // This context is CPU 0's idle thread, and dispatching an idle thread
    // always restarts it at idle_loop_arm64, so a block here would abandon
    // the rest of the boot. Loading reads ext2, and every read waits for a
    // VirtIO completion, so the loader runs on its own kernel thread and
    // this context becomes CPU 0's idle loop.
    #[cfg(feature = "testing")]
    if device_count > 0 {
        serial_println!("[test] Loading test binaries from ext2...");
        if let Err(e) = kernel::task::kthread::kthread_run(run_test_loader, "test-loader") {
            serial_println!("[test] Failed to start the test loader thread: {:?}", e);
        }
        // CPU 0 can now run test processes. Once the pin drops, a tick could
        // switch this idle context away for good, so keep IRQs masked until
        // CPU 0's softirq daemon has been published.
        unsafe {
            kernel::arch_impl::aarch64::cpu::Aarch64Cpu::disable_interrupts();
        }
        release_boot_preempt_pin();
        unsafe {
            kernel::arch_impl::aarch64::cpu::Aarch64Cpu::enable_interrupts();
        }
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
            kernel::net::drain_loopback_from_idle();
        }
    }

    // Launch userspace init from the pre-loaded ELF (read before SMP bring-up).
    // If the pre-load succeeded, use the cached bytes; otherwise fall back to
    // reading from ext2 now (may fail on Parallels with 8 CPUs) or test disk.
    if device_count > 0 {
        if let Some(elf_data) = init_elf {
            serial_println!("[boot] Launching init from pre-loaded ELF...");
            match init_launch.launch(elf_data) {
                Err(e) => {
                    serial_println!("[boot] Failed to launch pre-loaded init: {}", e);
                    // A probe, program or suite binary that would not launch: run
                    // the default /sbin/init in its place.
                    if runs_own_binary && boot_mode != BootMode::Default {
                        serial_println!(
                            "[boot] Ignoring boot mode {:?}: {} could not be launched; running /sbin/init",
                            boot_mode.name(),
                            init_launch.path
                        );
                        match read_init_from_ext2("/sbin/init") {
                            Ok(elf_data) => match InitLaunch::default_init().launch(elf_data) {
                                Err(e) => serial_println!("[boot] Failed to launch /sbin/init: {}", e),
                                Ok(never) => match never {},
                            },
                            Err(e) => serial_println!("[boot] Failed to read /sbin/init: {}", e),
                        }
                    }
                    serial_println!("[boot] Loading userspace init_shell from test disk...");
                    match kernel::boot::test_disk::run_userspace_from_disk("init_shell") {
                        Err(e) => {
                            serial_println!("[boot] Failed to load init_shell: {}", e);
                            serial_println!("[boot] Falling back to kernel shell...");
                        }
                        Ok(never) => match never {},
                    }
                }
                Ok(never) => match never {},
            }
        } else {
            // Pre-load was not attempted or failed — try ext2 now as a last resort
            // (single-CPU machines or QEMU where SMP interference isn't a concern).
            serial_println!("[boot] No pre-loaded init — attempting ext2 read post-SMP...");
            match read_init_from_ext2(&init_launch.path) {
                Ok(elf_data) => {
                    serial_println!("[boot] Late ext2 read succeeded, launching init...");
                    match init_launch.launch(elf_data) {
                        Err(e) => {
                            serial_println!("[boot] Failed to launch init: {}", e);
                            serial_println!(
                                "[boot] Loading userspace init_shell from test disk..."
                            );
                            match kernel::boot::test_disk::run_userspace_from_disk("init_shell") {
                                Err(e) => {
                                    serial_println!("[boot] Failed to load init_shell: {}", e);
                                    serial_println!("[boot] Falling back to kernel shell...");
                                }
                                Ok(never) => match never {},
                            }
                        }
                        Ok(never) => match never {},
                    }
                }
                Err(e) => {
                    serial_println!("[boot] Failed to load init from ext2: {}", e);
                    serial_println!("[boot] Loading userspace init_shell from test disk...");
                    match kernel::boot::test_disk::run_userspace_from_disk("init_shell") {
                        Err(e) => {
                            serial_println!("[boot] Failed to load init_shell: {}", e);
                            serial_println!("[boot] Falling back to kernel shell...");
                        }
                        Ok(never) => match never {},
                    }
                }
            }
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
        kernel::net::drain_loopback_from_idle();
    }
}

/// The testing profile's boot continuation, run on the `test-loader` kernel
/// thread: load the test binaries, then print the markers the boot gate reads.
#[cfg(target_arch = "aarch64")]
#[cfg(feature = "testing")]
#[cfg_attr(
    any(
        feature = "kthread_test_only",
        feature = "kthread_stress_test",
        feature = "workqueue_test_only"
    ),
    allow(dead_code)
)]
fn run_test_loader() {
    // Each create_user_process() adds a thread to the ready queue, and without
    // a preempt pin the timer would hand this CPU to those test processes
    // between binaries, stretching loading to tens of seconds. The pin keeps
    // the loader on its CPU except while it sleeps on a block read, the same
    // shape as a syscall: the completion wait releases the pin while it
    // sleeps and retakes it on wake, on whichever CPU that is.
    kernel::per_cpu_aarch64::preempt_disable();
    // Tests start running and exiting while later binaries are still being
    // read, so the suite only counts as finished once loading has ended.
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::begin_loading();
    load_test_binaries_from_ext2();
    #[cfg(feature = "btrt")]
    kernel::test_framework::btrt::end_loading();
    kernel::per_cpu_aarch64::preempt_enable();
    boot_screen::stage(Stage::StartingPid1);
    serial_println!("[test] Test processes loaded - will run via timer interrupts");
    serial_println!("[test] Entering scheduler idle loop");
    // The prompt signals to the test harness that boot is complete.
    serial_print!("breenix> ");
}

/// Load test binaries from ext2 filesystem and create userspace processes.
///
/// Each test binary is loaded from /bin/<name>.elf, parsed as ELF, and scheduled
/// via create_user_process(). The scheduler will run them alongside init_shell.
#[cfg(target_arch = "aarch64")]
#[cfg(feature = "testing")]
#[cfg_attr(
    any(
        feature = "kthread_test_only",
        feature = "kthread_stress_test",
        feature = "workqueue_test_only"
    ),
    allow(dead_code)
)]
fn load_test_binaries_from_ext2() {
    use alloc::format;
    use alloc::string::String;

    // Use the canonical shared test binary list (see boot::test_list)
    let test_binaries = kernel::boot::test_list::TEST_BINARIES;

    let mut loaded = 0;
    let mut failed = 0;
    // A testing kernel never launches init, so the boot tests' ProcessContext
    // cohort runs here instead: once, with the first test process published
    // and not yet runnable, the state `launch_init_from_elf` gives it. The
    // loader's preempt pin is released around it: the cohort's lock oracles
    // need this CPU to dispatch their network work while the loader waits.
    #[cfg(feature = "boot_tests")]
    let mut process_context_pending = true;

    // Search paths for test binaries - try each in order
    let search_dirs = ["/bin", "/usr/local/cbin", "/usr/local/test/bin", "/sbin"];

    for (index, name) in test_binaries.iter().enumerate() {
        // Show which program is loading, so a hang names it on screen.
        boot_screen::set_status(format_args!(
            "Loading test programs {} of {}: {}",
            index + 1,
            test_binaries.len(),
            name
        ));
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
            let mut found_entry = None;
            for dir in &search_dirs {
                let path = format!("{}/{}", dir, name);
                if let Ok(num) = fs.resolve_path(&path) {
                    found_entry = Some((num, path));
                    break;
                }
            }

            let (inode_num, resolved_path) = match found_entry {
                Some(entry) => entry,
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

            match fs.read_file_content_coherent(inode_num, &inode) {
                Ok(data) => data,
                Err(e) => {
                    serial_println!("[test] Failed to read {}: {}", resolved_path, e);
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
        let created = kernel::process::creation::create_user_process_before_run(
            String::from(*name),
            &elf_data,
            |_| {
                #[cfg(feature = "boot_tests")]
                if core::mem::take(&mut process_context_pending) {
                    kernel::per_cpu_aarch64::preempt_enable();
                    let failures = kernel::test_framework::advance_to_stage(
                        kernel::test_framework::TestStage::ProcessContext,
                    );
                    kernel::per_cpu_aarch64::preempt_disable();
                    if failures > 0 {
                        serial_println!("[boot_tests] {} ProcessContext test(s) failed", failures);
                    }
                }
            },
        );
        match created {
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
                    kernel::test_framework::btrt::fail(
                        test_id,
                        kernel::test_framework::btrt::BtrtErrorCode::NoExec,
                        0,
                    );
                }
                failed += 1;
            }
        }
    }

    serial_println!(
        "[test] Loaded {}/{} test binaries ({} failed, {} not found)",
        loaded,
        test_binaries.len(),
        failed,
        test_binaries.len() - loaded - failed
    );
    boot_screen::set_status(format_args!(
        "Loaded {} test programs ({} failed); running them",
        loaded, failed
    ));
    kernel::task::userspace_completion::start();
}

/// Initialize the scheduler with an idle thread (ARM64)
#[cfg(target_arch = "aarch64")]
fn init_scheduler() {
    use alloc::boxed::Box;
    use alloc::string::String;
    use kernel::memory::arch_stub::VirtAddr;
    use kernel::per_cpu_aarch64;
    use kernel::task::scheduler;
    use kernel::task::thread::{Thread, ThreadPrivilege, ThreadState};

    // CPU 0 boot stack top address.
    // On QEMU: boot.S sets SP to HHDM_BASE + STACK_REGION_BASE + STACK_SIZE
    // On Parallels: UEFI loader sets SP to 0x42000000, then HHDM switch adds HHDM_BASE
    // Use platform detection to pick the right boot stack address.
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let (boot_stack_top, boot_stack_bottom) =
        if kernel::platform_config::is_qemu() || kernel::platform_config::is_vmware() {
            (
                VirtAddr::new(kernel::arch_impl::aarch64::constants::percpu_kernel_stack_top(0)),
                VirtAddr::new(kernel::arch_impl::aarch64::constants::percpu_kernel_stack_bottom(0)),
            )
        } else {
            // Parallels: UEFI loader stack at 0x42000000 (phys), now at HHDM
            // The loader maps the full 2MB region without overflow protection.
            const PARALLELS_STACK_TOP_PHYS: u64 = 0x4200_0000;
            const PARALLELS_STACK_SIZE: u64 = 0x20_0000;
            (
                VirtAddr::new(HHDM_BASE + PARALLELS_STACK_TOP_PHYS),
                VirtAddr::new(HHDM_BASE + PARALLELS_STACK_TOP_PHYS - PARALLELS_STACK_SIZE),
            )
        };
    let dummy_tls = VirtAddr::zero();

    // Create the idle task (thread ID 0)
    let mut idle_task = Box::new(Thread::new(
        String::from("swapper/0"), // Linux convention: swapper/0 is the idle task
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

    // Mark as running, and has_started=true since boot code is already executing.
    // The id stays the one `Thread::new` allocated: 0 is the no-thread sentinel
    // and is never a live thread id.
    idle_task.state = ThreadState::Running;
    idle_task.has_started = true; // CRITICAL: Boot thread is already running, not waiting for first entry

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
        kernel::net::drain_loopback_from_idle();
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
        serial_println!(
            "[test] clock_gettime() returned: {}.{:09} seconds",
            timespec[0],
            timespec[1]
        );
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
    serial_println!(
        "[test] unknown syscall returned: {} (expected -38 ENOSYS)",
        enosys
    );

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
    #[allow(dead_code)] // Fields are used via write_volatile
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
            0xd2800801, // movk x1, #0x4100, lsl #16    (x1 = 0x41000040)
            0xf2a82001, // mov x8, #1 (write syscall)
            0xd2800028, // mov x0, #1 (fd = stdout)
            0xd2800020, // mov x2, #24 (message length)
            0xd2800302, // svc #0
            0xd4000001, // mov x8, #0 (exit syscall)
            0xd2800008, // mov x0, #42 (exit code)
            0xd2800540, // svc #0
            0xd4000001,
            // Just in case exit doesn't work, spin forever
            // b . (branch to self)
            0x14000000, 0x14000000, 0x14000000, 0x14000000, 0x14000000, 0x14000000,
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
    // Initialize VirtIO GPU backend.
    // GPU PCI is initialized earlier in drivers::init() — only init GPU MMIO
    // if PCI is not available (QEMU virt platform).
    if !kernel::drivers::virtio::gpu_pci::is_initialized() {
        kernel::drivers::virtio::gpu_mmio::init()?;
    }

    arm64_fb::init_shell_framebuffer()?;

    // Get framebuffer dimensions
    let (width, height) = arm64_fb::dimensions().ok_or("Failed to get framebuffer dimensions")?;
    serial_println!("[graphics] Framebuffer: {}x{}", width, height);

    let left_width = width / 2;

    // The boot screen (or the fbconsole=log console) is drawn once the
    // framebuffer is double-buffered; see kernel_main.

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

    serial_println!("[graphics] Split-screen terminal UI initialized");
    Ok(())
}

/// Initialize GOP framebuffer display with split-screen terminal UI.
///
/// Called after init_gop_framebuffer() when a UEFI GOP framebuffer is available.
/// Draws the same layout as VirtIO init_graphics() but without VirtIO-specific init.
#[cfg(target_arch = "aarch64")]
fn init_gop_display() -> Result<(), &'static str> {
    // Get framebuffer dimensions
    let (width, height) = arm64_fb::dimensions().ok_or("Failed to get framebuffer dimensions")?;
    serial_println!("[graphics] GOP Framebuffer: {}x{}", width, height);

    let left_width = width / 2;

    // The boot screen (or the fbconsole=log console) is drawn once the
    // framebuffer is double-buffered; see kernel_main.

    // Initialize particle system for left pane
    let margin = 10;
    particles::start_animation(
        margin as i32,
        margin as i32,
        (left_width - margin) as i32,
        (height - margin) as i32,
    );
    serial_println!("[graphics] Particle system initialized");

    // Initialize the render queue for deferred framebuffer rendering
    kernel::graphics::render_queue::init();

    serial_println!("[graphics] GOP split-screen terminal UI initialized");
    Ok(())
}

/// Panic handler
#[cfg(target_arch = "aarch64")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Mask IRQ and FIQ on this CPU, as Linux's panic() masks interrupts: a timer
    // tick taken between the serial dumps below could preempt the panicking
    // thread and switch away from it, leaving the dumps and the screen unfinished.
    // Each serial print already masks them while it writes.
    unsafe {
        core::arch::asm!("msr DAIFSet, #0x3", options(nomem, nostack));
    }

    serial_println!();
    serial_println!("========================================");
    serial_println!("  KERNEL PANIC!");
    serial_println!("========================================");
    serial_println!("{}", info);
    serial_println!();

    // The screen drawn below shows the log as it stands now, ending with this
    // banner, not the dumps printed in between.
    boot_screen::capture_panic_log();

    // Failure-capture PR-4: the bounded, lock-free BXCAP record.
    //
    // ORDER. It goes after the banner because a reader wants the panic
    // message immediately above the state that explains it, and it goes
    // BEFORE the `wfi` loop below, which does not return. It is emitted
    // through `raw_serial_char`, which takes no lock, so it lands even when
    // the counters dump beneath it would not.
    //
    // The two words are the panic site's line and column, which is what
    // `[BXCAP:EDGE a0= a1=]` can carry without formatting anything.
    let (panic_line, panic_column) = match info.location() {
        Some(location) => (location.line() as u64, location.column() as u64),
        None => (0, 0),
    };
    kernel::capture::emit(kernel::capture::Edge::Panic, panic_line, panic_column);

    // Turn 3 net triage: panic is the only reliable serial-side rendezvous
    // when the RX probe triggers the CPU0 regression before userspace can dump.
    //
    // KEPT, not replaced by the capture above. `[BXCAP:CNT]` emits at most
    // 32 nonzero counters (kernel/src/capture/sections.rs) where this dumps
    // the whole registry -- 196 of 196 on the aarch64 boot_tests profile, as
    // the committed baseline serial
    // docs/planning/green-program/failure-capture/serials/pr4-red/main-aarch64-panic-no-capture.txt
    // shows -- so dropping it would narrow the evidence on the one path that
    // has it today.
    kernel::tracing::output::trace_dump_counters();

    // Show the panic on screen after the serial records above, so drawing and
    // GPU commands cannot delay or lose them. It does not print or block on a
    // lock: skipped if the framebuffer lock is held, and left for the render or
    // presenter thread if the GPU lock stays held (possibly by this CPU).
    boot_screen::show_panic(info);

    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
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
