//! Canonical list of userspace test binaries.
//!
//! Both x86_64 and ARM64 reference this list when loading test binaries.
//! The binary *sources* differ per architecture (BXTEST disk on x86_64 vs
//! ext2 filesystem on ARM64), but the set of test names is shared.
//!
//! When adding a new userspace test binary, add its name here so both
//! architectures will load it.

/// Canonical list of userspace test binaries.
///
/// Both x86_64 and ARM64 load from this list. The x86_64 path loads
/// binaries individually via `userspace_test::get_test_binary()` in
/// `main.rs` / `test_exec.rs` (see those files for the per-binary
/// loading pattern). The ARM64 path iterates this list directly in
/// `load_test_binaries_from_ext2()`.
///
/// Binaries not present on a given architecture's disk/filesystem are
/// silently skipped.
pub const TEST_BINARIES: &[&str] = &[
    // Core functionality tests
    "hello_time",
    "clock_gettime_test",
    "brk_test",
    "test_mmap",
    "syscall_diagnostic_test",
    // Signal tests
    "signal_test",
    "signal_handler_test",
    "signal_return_test",
    "signal_regs_test",
    "sigaltstack_test",
    "sigchld_test",
    "pause_test",
    "sigsuspend_test",
    "signal_exec_test",
    "signal_fork_test",
    "ctrl_c_test",
    // IPC tests
    "pipe_test",
    "unix_socket_test",
    "dup_test",
    "fcntl_test",
    "cloexec_test",
    "pipe2_test",
    "shell_pipe_test",
    "waitpid_test",
    "wnohang_timing_test",
    "poll_test",
    "select_test",
    "nonblock_test",
    // TTY / session tests
    "tty_test",
    "session_test",
    // Filesystem tests
    "file_read_test",
    "getdents_test",
    "lseek_test",
    "fs_write_test",
    "fs_rename_test",
    "fs_large_file_test",
    "fs_directory_test",
    "fs_link_test",
    "access_test",
    "devfs_test",
    "cwd_test",
    "exec_from_ext2_test",
    "fs_block_alloc_test",
    // Coreutils tests
    "true_test",
    "false_test",
    "head_test",
    "tail_test",
    "wc_test",
    "which_test",
    "cat_test",
    "ls_test",
    // Rust std library test (installed as hello_world.elf on ext2)
    "hello_world",
    // musl libc C programs (cross-compiled with musl libc for aarch64)
    "hello_musl",
    "env_musl_test",
    "uname_musl_test",
    "rlimit_musl_test",
    // Fork / CoW tests
    "fork_memory_test",
    "fork_state_test",
    "fork_pending_signal_test",
    "cow_signal_test",
    "cow_cleanup_test",
    "cow_sole_owner_test",
    "cow_stress_test",
    "cow_readonly_test",
    // Argv / exec tests
    "argv_test",
    "exec_argv_test",
    "exec_stack_argv_test",
    // Graphics tests
    "fbinfo_test",
    // Network tests (depend on virtio-net)
    "udp_socket_test",
    "tcp_socket_test",
    "dns_test",
    "http_test",
];
