//! Boot test catalog: ID constants and metadata for BTRT entries.
//!
//! Each test milestone has a unique ID, name, and category.
//! IDs are stable across releases -- never reuse or renumber.
//!
//! # ID Ranges
//!
//! - 0-99: Kernel initialization milestones
//! - 100-199: Subsystem initialization
//! - 200-299: Userspace binary load/exec
//! - 300-499: Userspace test results
//! - 500-511: Reserved

/// Category of a boot test milestone (wire protocol -- all variants required for host-side parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootTestCategory {
    /// Core kernel init (GDT, IDT, memory, serial, etc.)
    KernelInit = 0,
    /// Driver/hardware init (PCI, NIC, disk, etc.)
    DriverInit = 1,
    /// Subsystem init (scheduler, FS, network stack, etc.)
    Subsystem = 2,
    /// Userspace binary load and exec
    UserspaceExec = 3,
    /// Userspace test result (exit code check)
    UserspaceResult = 4,
}

/// A boot test definition in the catalog.
pub struct BootTestDef {
    pub id: u16,
    pub name: &'static str,
    pub category: BootTestCategory,
}

// =============================================================================
// Kernel Initialization (0-99)
// =============================================================================

pub const KERNEL_ENTRY: u16 = 0;
pub const SERIAL_INIT: u16 = 1;
pub const GDT_IDT_INIT: u16 = 2;
pub const PER_CPU_INIT: u16 = 3;
pub const MEMORY_INIT: u16 = 4;
pub const HEAP_INIT: u16 = 5;
pub const FRAME_ALLOC_INIT: u16 = 6;
pub const INTERRUPTS_ENABLED: u16 = 7;
pub const TRACING_INIT: u16 = 8;
pub const TIMER_INIT: u16 = 9;

// =============================================================================
// Driver Initialization (10-29, within kernel init range)
// =============================================================================

pub const PCI_ENUMERATION: u16 = 10;
pub const NIC_INIT: u16 = 11;
pub const VIRTIO_BLK_INIT: u16 = 12;

// =============================================================================
// Subsystem Initialization (100-199)
// =============================================================================

pub const SCHEDULER_INIT: u16 = 100;
pub const KTHREAD_SUBSYSTEM: u16 = 101;
pub const WORKQUEUE_INIT: u16 = 102;
pub const FILESYSTEM_INIT: u16 = 103;
pub const EXT2_MOUNT: u16 = 104;
pub const NETWORK_STACK_INIT: u16 = 105;
pub const PROCFS_INIT: u16 = 106;
pub const PIPE_SUBSYSTEM: u16 = 107;

// =============================================================================
// Userspace Execution (200-299)
// =============================================================================

pub const USERSPACE_PROCESS_CREATE: u16 = 200;
pub const USERSPACE_ELF_LOAD: u16 = 201;
pub const USERSPACE_FIRST_INSTRUCTION: u16 = 202;
pub const USERSPACE_FIRST_SYSCALL: u16 = 203;

// =============================================================================
// Boot Test Framework (250-259) -- parallel test framework results
// =============================================================================

pub const BOOT_TESTS_START: u16 = 250;
pub const BOOT_TESTS_COMPLETE: u16 = 251;

// =============================================================================
// ARM64-Specific (30-49, within kernel init range)
// =============================================================================

pub const AARCH64_MMU_INIT: u16 = 30;
pub const AARCH64_EXCEPTION_VECTORS: u16 = 31;
pub const AARCH64_GIC_INIT: u16 = 32;
pub const AARCH64_TIMER_INIT: u16 = 33;
pub const AARCH64_UART_INIT: u16 = 34;
pub const AARCH64_FRAMEBUFFER_INIT: u16 = 35;

// =============================================================================
// Userspace Test Results (300-399)
// =============================================================================

pub const UTEST_HELLO_TIME: u16 = 300;
pub const UTEST_CLOCK_GETTIME: u16 = 301;
pub const UTEST_BRK: u16 = 302;
pub const UTEST_MMAP: u16 = 303;
pub const UTEST_SYSCALL_DIAGNOSTIC: u16 = 304;
pub const UTEST_SIGNAL: u16 = 305;
pub const UTEST_SIGNAL_REGS: u16 = 306;
pub const UTEST_SIGALTSTACK: u16 = 307;
pub const UTEST_PIPE: u16 = 308;
pub const UTEST_UNIX_SOCKET: u16 = 309;
pub const UTEST_SIGCHLD: u16 = 310;
pub const UTEST_PAUSE: u16 = 311;
pub const UTEST_SIGSUSPEND: u16 = 312;
pub const UTEST_DUP: u16 = 313;
pub const UTEST_FCNTL: u16 = 314;
pub const UTEST_CLOEXEC: u16 = 315;
pub const UTEST_PIPE2: u16 = 316;
pub const UTEST_SHELL_PIPE: u16 = 317;
pub const UTEST_SIGNAL_EXEC: u16 = 318;
pub const UTEST_WAITPID: u16 = 319;
pub const UTEST_SIGNAL_FORK: u16 = 320;
pub const UTEST_WNOHANG_TIMING: u16 = 321;
pub const UTEST_POLL: u16 = 322;
pub const UTEST_SELECT: u16 = 323;
pub const UTEST_NONBLOCK: u16 = 324;
pub const UTEST_TTY: u16 = 325;
pub const UTEST_SESSION: u16 = 326;
pub const UTEST_FILE_READ: u16 = 327;
pub const UTEST_GETDENTS: u16 = 328;
pub const UTEST_LSEEK: u16 = 329;
pub const UTEST_CTRL_C: u16 = 330;
pub const UTEST_FORK_MEMORY: u16 = 331;
pub const UTEST_FORK_STATE: u16 = 332;
pub const UTEST_FORK_PENDING_SIGNAL: u16 = 333;
pub const UTEST_COW_SIGNAL: u16 = 334;
pub const UTEST_COW_CLEANUP: u16 = 335;
pub const UTEST_COW_SOLE_OWNER: u16 = 336;
pub const UTEST_COW_STRESS: u16 = 337;
pub const UTEST_COW_READONLY: u16 = 338;
pub const UTEST_ARGV: u16 = 339;
pub const UTEST_EXEC_ARGV: u16 = 340;
pub const UTEST_EXEC_STACK_ARGV: u16 = 341;
pub const UTEST_FBINFO: u16 = 342;
pub const UTEST_UDP_SOCKET: u16 = 343;
pub const UTEST_TCP_SOCKET: u16 = 344;
pub const UTEST_DNS: u16 = 345;
pub const UTEST_HTTP: u16 = 346;
pub const UTEST_TRUE_COREUTIL: u16 = 347;
pub const UTEST_FALSE_COREUTIL: u16 = 348;
pub const UTEST_HEAD_COREUTIL: u16 = 349;
pub const UTEST_TAIL_COREUTIL: u16 = 350;
pub const UTEST_WC_COREUTIL: u16 = 351;
pub const UTEST_WHICH_COREUTIL: u16 = 352;
pub const UTEST_CAT_COREUTIL: u16 = 353;
pub const UTEST_LS_COREUTIL: u16 = 354;
pub const UTEST_HELLO_STD_REAL: u16 = 355;
pub const UTEST_FIFO: u16 = 356;
pub const UTEST_KILL_PROCESS_GROUP: u16 = 357;
pub const UTEST_EXEC_FROM_EXT2: u16 = 358;
pub const UTEST_FS_BLOCK_ALLOC: u16 = 359;
pub const UTEST_FS_WRITE: u16 = 360;
pub const UTEST_FS_RENAME: u16 = 361;
pub const UTEST_FS_LARGE_FILE: u16 = 362;
pub const UTEST_FS_DIRECTORY: u16 = 363;
pub const UTEST_FS_LINK: u16 = 364;
pub const UTEST_ACCESS: u16 = 365;
pub const UTEST_DEVFS: u16 = 366;
pub const UTEST_CWD: u16 = 367;
pub const UTEST_SIGNAL_KILL: u16 = 368;
pub const UTEST_SIGNAL_RETURN: u16 = 369;
pub const UTEST_SIGNAL_HANDLER: u16 = 370;
pub const UTEST_SYSCALL_ENOSYS: u16 = 371;
pub const UTEST_UNIX_NAMED_SOCKET: u16 = 372;
pub const UTEST_PIPE_FORK: u16 = 373;
pub const UTEST_PIPE_CONCURRENT: u16 = 374;
pub const UTEST_JOB_CONTROL: u16 = 375;

// =============================================================================
// Full Catalog
// =============================================================================

/// Complete catalog of boot test milestones.
///
/// This is the single source of truth for test ID → name mappings.
/// The xtask host-side catalog (`xtask/src/btrt_catalog.rs`) must be
/// kept in sync with this list.
pub static CATALOG: &[BootTestDef] = &[
    // Kernel init
    BootTestDef { id: KERNEL_ENTRY, name: "kernel_entry", category: BootTestCategory::KernelInit },
    BootTestDef { id: SERIAL_INIT, name: "serial_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: GDT_IDT_INIT, name: "gdt_idt_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: PER_CPU_INIT, name: "per_cpu_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: MEMORY_INIT, name: "memory_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: HEAP_INIT, name: "heap_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: FRAME_ALLOC_INIT, name: "frame_alloc_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: INTERRUPTS_ENABLED, name: "interrupts_enabled", category: BootTestCategory::KernelInit },
    BootTestDef { id: TRACING_INIT, name: "tracing_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: TIMER_INIT, name: "timer_init", category: BootTestCategory::KernelInit },
    // Drivers
    BootTestDef { id: PCI_ENUMERATION, name: "pci_enumeration", category: BootTestCategory::DriverInit },
    BootTestDef { id: NIC_INIT, name: "nic_init", category: BootTestCategory::DriverInit },
    BootTestDef { id: VIRTIO_BLK_INIT, name: "virtio_blk_init", category: BootTestCategory::DriverInit },
    // Subsystems
    BootTestDef { id: SCHEDULER_INIT, name: "scheduler_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: KTHREAD_SUBSYSTEM, name: "kthread_subsystem", category: BootTestCategory::Subsystem },
    BootTestDef { id: WORKQUEUE_INIT, name: "workqueue_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: FILESYSTEM_INIT, name: "filesystem_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: EXT2_MOUNT, name: "ext2_mount", category: BootTestCategory::Subsystem },
    BootTestDef { id: NETWORK_STACK_INIT, name: "network_stack_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: PROCFS_INIT, name: "procfs_init", category: BootTestCategory::Subsystem },
    BootTestDef { id: PIPE_SUBSYSTEM, name: "pipe_subsystem", category: BootTestCategory::Subsystem },
    // Userspace exec
    BootTestDef { id: USERSPACE_PROCESS_CREATE, name: "userspace_process_create", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_ELF_LOAD, name: "userspace_elf_load", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_FIRST_INSTRUCTION, name: "userspace_first_instruction", category: BootTestCategory::UserspaceExec },
    BootTestDef { id: USERSPACE_FIRST_SYSCALL, name: "userspace_first_syscall", category: BootTestCategory::UserspaceExec },
    // Boot tests framework
    BootTestDef { id: BOOT_TESTS_START, name: "boot_tests_start", category: BootTestCategory::Subsystem },
    BootTestDef { id: BOOT_TESTS_COMPLETE, name: "boot_tests_complete", category: BootTestCategory::Subsystem },
    // ARM64
    BootTestDef { id: AARCH64_MMU_INIT, name: "aarch64_mmu_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_EXCEPTION_VECTORS, name: "aarch64_exception_vectors", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_GIC_INIT, name: "aarch64_gic_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_TIMER_INIT, name: "aarch64_timer_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_UART_INIT, name: "aarch64_uart_init", category: BootTestCategory::KernelInit },
    BootTestDef { id: AARCH64_FRAMEBUFFER_INIT, name: "aarch64_framebuffer_init", category: BootTestCategory::KernelInit },
    // Userspace test results
    BootTestDef { id: UTEST_HELLO_TIME, name: "utest_hello_time", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_CLOCK_GETTIME, name: "utest_clock_gettime", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_BRK, name: "utest_brk", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_MMAP, name: "utest_mmap", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SYSCALL_DIAGNOSTIC, name: "utest_syscall_diagnostic", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL, name: "utest_signal", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_REGS, name: "utest_signal_regs", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGALTSTACK, name: "utest_sigaltstack", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_PIPE, name: "utest_pipe", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_UNIX_SOCKET, name: "utest_unix_socket", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGCHLD, name: "utest_sigchld", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_PAUSE, name: "utest_pause", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGSUSPEND, name: "utest_sigsuspend", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_DUP, name: "utest_dup", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FCNTL, name: "utest_fcntl", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_CLOEXEC, name: "utest_cloexec", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_PIPE2, name: "utest_pipe2", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SHELL_PIPE, name: "utest_shell_pipe", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_EXEC, name: "utest_signal_exec", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_WAITPID, name: "utest_waitpid", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_FORK, name: "utest_signal_fork", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_WNOHANG_TIMING, name: "utest_wnohang_timing", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_POLL, name: "utest_poll", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SELECT, name: "utest_select", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_NONBLOCK, name: "utest_nonblock", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_TTY, name: "utest_tty", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SESSION, name: "utest_session", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FILE_READ, name: "utest_file_read", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_GETDENTS, name: "utest_getdents", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_LSEEK, name: "utest_lseek", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_CTRL_C, name: "utest_ctrl_c", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FORK_MEMORY, name: "utest_fork_memory", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FORK_STATE, name: "utest_fork_state", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FORK_PENDING_SIGNAL, name: "utest_fork_pending_signal", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_COW_SIGNAL, name: "utest_cow_signal", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_COW_CLEANUP, name: "utest_cow_cleanup", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_COW_SOLE_OWNER, name: "utest_cow_sole_owner", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_COW_STRESS, name: "utest_cow_stress", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_COW_READONLY, name: "utest_cow_readonly", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_ARGV, name: "utest_argv", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_EXEC_ARGV, name: "utest_exec_argv", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_EXEC_STACK_ARGV, name: "utest_exec_stack_argv", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FBINFO, name: "utest_fbinfo", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_UDP_SOCKET, name: "utest_udp_socket", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_TCP_SOCKET, name: "utest_tcp_socket", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_DNS, name: "utest_dns", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_HTTP, name: "utest_http", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_TRUE_COREUTIL, name: "utest_true_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FALSE_COREUTIL, name: "utest_false_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_HEAD_COREUTIL, name: "utest_head_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_TAIL_COREUTIL, name: "utest_tail_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_WC_COREUTIL, name: "utest_wc_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_WHICH_COREUTIL, name: "utest_which_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_CAT_COREUTIL, name: "utest_cat_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_LS_COREUTIL, name: "utest_ls_coreutil", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_HELLO_STD_REAL, name: "utest_hello_std_real", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FIFO, name: "utest_fifo", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_KILL_PROCESS_GROUP, name: "utest_kill_process_group", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_EXEC_FROM_EXT2, name: "utest_exec_from_ext2", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_BLOCK_ALLOC, name: "utest_fs_block_alloc", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_WRITE, name: "utest_fs_write", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_RENAME, name: "utest_fs_rename", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_LARGE_FILE, name: "utest_fs_large_file", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_DIRECTORY, name: "utest_fs_directory", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_FS_LINK, name: "utest_fs_link", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_ACCESS, name: "utest_access", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_DEVFS, name: "utest_devfs", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_CWD, name: "utest_cwd", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_KILL, name: "utest_signal_kill", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_RETURN, name: "utest_signal_return", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SIGNAL_HANDLER, name: "utest_signal_handler", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_SYSCALL_ENOSYS, name: "utest_syscall_enosys", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_UNIX_NAMED_SOCKET, name: "utest_unix_named_socket", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_PIPE_FORK, name: "utest_pipe_fork", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_PIPE_CONCURRENT, name: "utest_pipe_concurrent", category: BootTestCategory::UserspaceResult },
    BootTestDef { id: UTEST_JOB_CONTROL, name: "utest_job_control", category: BootTestCategory::UserspaceResult },
];

/// Look up a test name by ID.
pub fn test_name(id: u16) -> &'static str {
    for def in CATALOG {
        if def.id == id {
            return def.name;
        }
    }
    "unknown"
}

/// Map a userspace binary name (as used on ext2 disk) to a BTRT test ID.
///
/// Used by ARM64's `load_test_binaries_from_ext2()` which identifies tests
/// by name rather than by function call.
pub fn utest_name_to_id(name: &str) -> Option<u16> {
    match name {
        "hello_time" => Some(UTEST_HELLO_TIME),
        "clock_gettime_test" => Some(UTEST_CLOCK_GETTIME),
        "brk_test" => Some(UTEST_BRK),
        "test_mmap" => Some(UTEST_MMAP),
        "syscall_diagnostic_test" => Some(UTEST_SYSCALL_DIAGNOSTIC),
        "signal_test" => Some(UTEST_SIGNAL),
        "signal_regs_test" => Some(UTEST_SIGNAL_REGS),
        "sigaltstack_test" => Some(UTEST_SIGALTSTACK),
        "pipe_test" => Some(UTEST_PIPE),
        "unix_socket_test" => Some(UTEST_UNIX_SOCKET),
        "sigchld_test" => Some(UTEST_SIGCHLD),
        "pause_test" => Some(UTEST_PAUSE),
        "sigsuspend_test" => Some(UTEST_SIGSUSPEND),
        "dup_test" => Some(UTEST_DUP),
        "fcntl_test" => Some(UTEST_FCNTL),
        "cloexec_test" => Some(UTEST_CLOEXEC),
        "pipe2_test" => Some(UTEST_PIPE2),
        "shell_pipe_test" => Some(UTEST_SHELL_PIPE),
        "signal_exec_test" => Some(UTEST_SIGNAL_EXEC),
        "waitpid_test" => Some(UTEST_WAITPID),
        "signal_fork_test" => Some(UTEST_SIGNAL_FORK),
        "wnohang_timing_test" => Some(UTEST_WNOHANG_TIMING),
        "poll_test" => Some(UTEST_POLL),
        "select_test" => Some(UTEST_SELECT),
        "nonblock_test" => Some(UTEST_NONBLOCK),
        "tty_test" => Some(UTEST_TTY),
        "session_test" => Some(UTEST_SESSION),
        "file_read_test" => Some(UTEST_FILE_READ),
        "getdents_test" => Some(UTEST_GETDENTS),
        "lseek_test" => Some(UTEST_LSEEK),
        "ctrl_c_test" => Some(UTEST_CTRL_C),
        "fork_memory_test" => Some(UTEST_FORK_MEMORY),
        "fork_state_test" => Some(UTEST_FORK_STATE),
        "fork_pending_signal_test" => Some(UTEST_FORK_PENDING_SIGNAL),
        "cow_signal_test" => Some(UTEST_COW_SIGNAL),
        "cow_cleanup_test" => Some(UTEST_COW_CLEANUP),
        "cow_sole_owner_test" => Some(UTEST_COW_SOLE_OWNER),
        "cow_stress_test" => Some(UTEST_COW_STRESS),
        "cow_readonly_test" => Some(UTEST_COW_READONLY),
        "argv_test" => Some(UTEST_ARGV),
        "exec_argv_test" => Some(UTEST_EXEC_ARGV),
        "exec_stack_argv_test" => Some(UTEST_EXEC_STACK_ARGV),
        "fbinfo_test" => Some(UTEST_FBINFO),
        "udp_socket_test" => Some(UTEST_UDP_SOCKET),
        "tcp_socket_test" => Some(UTEST_TCP_SOCKET),
        "dns_test" => Some(UTEST_DNS),
        "http_test" => Some(UTEST_HTTP),
        "true_test" => Some(UTEST_TRUE_COREUTIL),
        "false_test" => Some(UTEST_FALSE_COREUTIL),
        "head_test" => Some(UTEST_HEAD_COREUTIL),
        "tail_test" => Some(UTEST_TAIL_COREUTIL),
        "wc_test" => Some(UTEST_WC_COREUTIL),
        "which_test" => Some(UTEST_WHICH_COREUTIL),
        "cat_test" => Some(UTEST_CAT_COREUTIL),
        "ls_test" => Some(UTEST_LS_COREUTIL),
        // hello_std_real is installed as hello_world on the ext2 disk
        "hello_world" | "hello_std_real" => Some(UTEST_HELLO_STD_REAL),
        "fifo_test" => Some(UTEST_FIFO),
        "kill_process_group_test" => Some(UTEST_KILL_PROCESS_GROUP),
        "exec_from_ext2_test" => Some(UTEST_EXEC_FROM_EXT2),
        "fs_block_alloc_test" => Some(UTEST_FS_BLOCK_ALLOC),
        "fs_write_test" => Some(UTEST_FS_WRITE),
        "fs_rename_test" => Some(UTEST_FS_RENAME),
        "fs_large_file_test" => Some(UTEST_FS_LARGE_FILE),
        "fs_directory_test" => Some(UTEST_FS_DIRECTORY),
        "fs_link_test" => Some(UTEST_FS_LINK),
        "access_test" => Some(UTEST_ACCESS),
        "devfs_test" => Some(UTEST_DEVFS),
        "cwd_test" => Some(UTEST_CWD),
        "signal_handler_test" => Some(UTEST_SIGNAL_HANDLER),
        "signal_return_test" => Some(UTEST_SIGNAL_RETURN),
        "syscall_enosys" => Some(UTEST_SYSCALL_ENOSYS),
        "unix_named_socket_test" => Some(UTEST_UNIX_NAMED_SOCKET),
        "pipe_fork_test" => Some(UTEST_PIPE_FORK),
        "pipe_concurrent_test" => Some(UTEST_PIPE_CONCURRENT),
        "job_control_test" => Some(UTEST_JOB_CONTROL),
        _ => None,
    }
}
