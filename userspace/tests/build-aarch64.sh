#!/bin/bash
set -e

# Add LLVM tools to PATH
SYSROOT=$(rustc --print sysroot)
HOST_TRIPLE=$(rustc -vV | grep host | cut -d' ' -f2)
LLVM_TOOLS_PATH="$SYSROOT/lib/rustlib/$HOST_TRIPLE/bin"
if [ -d "$LLVM_TOOLS_PATH" ]; then
    export PATH="$LLVM_TOOLS_PATH:$PATH"
fi

if ! command -v rust-objcopy &> /dev/null; then
    echo "ERROR: rust-objcopy not found"
    echo "Install llvm-tools-preview: rustup component add llvm-tools-preview"
    exit 1
fi

echo "========================================"
echo "  ARM64 USERSPACE BUILD"
echo "========================================"

# List of binaries to include (only those that are ARM64 compatible - no x86_64 inline asm)
# These are intended to populate /bin for ext2 init_shell use.
#
# EXCLUDED (x86_64 inline asm): brk_test, pipe_refcount_test, pipe_test, register_init_test,
# signal_regs_test, stdin_test, syscall_diagnostic_test, syscall_enosys, timer_test,
# poll_test, select_test, sigaltstack_test
#
BINARIES=(
    # ============================================================================
    # EXAMPLES - Demo programs and shell
    # ============================================================================
    "hello_world"
    "simple_exit"
    "hello_time"
    # "counter" - excluded: uses x86_64 inline asm (int 0x80 with rax/rdi/rsi/rdx)
    "spinner"
    "init_shell"

    # ============================================================================
    # COREUTILS - Standard POSIX utilities
    # ============================================================================
    "cat"
    "ls"
    "echo"
    "mkdir"
    "rmdir"
    "rm"
    "cp"
    "mv"
    "true"
    "false"
    "head"
    "tail"
    "wc"
    "which"
    "resolution"

    # ============================================================================
    # SERVICES - Daemons and network services
    # ============================================================================
    "telnetd"
    "init"

    # ============================================================================
    # TESTS - Process and fork tests
    # ============================================================================
    "fork_test"
    "fork_memory_test"
    "fork_state_test"
    "waitpid_test"
    "wnohang_timing_test"

    # ============================================================================
    # TESTS - Signal handling
    # ============================================================================
    "signal_test"
    "signal_handler_test"
    "signal_return_test"
    "signal_fork_test"
    "fork_pending_signal_test"
    "sigchld_test"
    "sigchld_job_test"
    "signal_exec_test"
    "signal_exec_check"
    "pause_test"
    "kill_process_group_test"
    "ctrl_c_test"
    "alarm_test"
    "sigsuspend_test"
    "itimer_test"

    # ============================================================================
    # TESTS - Pipe and IPC
    # ============================================================================
    "pipe_fork_test"
    "pipe_concurrent_test"
    "pipe2_test"
    "shell_pipe_test"
    "pipeline_test"
    "fifo_test"
    "unix_socket_test"
    "unix_named_socket_test"

    # ============================================================================
    # TESTS - Network sockets
    # ============================================================================
    "udp_socket_test"
    "tcp_socket_test"
    "tcp_blocking_test"
    "tcp_client_test"
    "blocking_recv_test"
    "concurrent_recv_stress"
    "nonblock_eagain_test"
    "dns_test"
    "http_test"

    # ============================================================================
    # TESTS - File descriptors
    # ============================================================================
    "dup_test"
    "fcntl_test"
    "cloexec_test"
    "nonblock_test"
    "lseek_test"
    "file_read_test"
    "getdents_test"

    # ============================================================================
    # TESTS - Filesystem operations
    # ============================================================================
    "fs_write_test"
    "fs_rename_test"
    "fs_large_file_test"
    "fs_directory_test"
    "fs_link_test"
    "fs_block_alloc_test"
    "access_test"
    "devfs_test"
    "cwd_test"
    "exec_from_ext2_test"

    # ============================================================================
    # TESTS - Memory management (COW)
    # ============================================================================
    "test_mmap"
    "cow_signal_test"
    "cow_cleanup_test"
    "cow_sole_owner_test"
    "cow_stress_test"
    "cow_readonly_test"
    "cow_oom_test"

    # ============================================================================
    # TESTS - TTY and job control
    # ============================================================================
    "tty_test"
    "job_control_test"
    "session_test"
    "job_table_test"
    "pty_test"

    # ============================================================================
    # TESTS - Exec and argv
    # ============================================================================
    "argv_test"
    "exec_argv_test"
    "exec_stack_argv_test"
    "clock_gettime_test"

    # ============================================================================
    # TESTS - Coreutil integration tests
    # ============================================================================
    "true_test"
    "false_test"
    "head_test"
    "tail_test"
    "wc_test"
    "which_test"
    "cat_test"
    "ls_test"
    "mkdir_argv_test"
    "cp_mv_argv_test"
    "echo_argv_test"
    "rm_argv_test"

    # ============================================================================
    # DEMOS - Interactive graphics (may have ARM64 issues)
    # ============================================================================
    "fbinfo_test"
    "demo"
    "bounce"
    "particles"
)

# Binaries that rely on the libbreenix runtime _start (no local _start)
# NOTE: resolution is NOT in this list because it has its own _start()
RUNTIME_BINS=(
    "cat"
    "ls"
    "echo"
    "mkdir"
    "rmdir"
    "rm"
    "cp"
    "mv"
    "true"
    "false"
    "head"
    "tail"
    "wc"
    "which"
)

# Create output directory for ARM64 binaries
mkdir -p aarch64

echo ""
echo "Building ${#BINARIES[@]} ARM64 userspace binaries..."

# Build each binary individually to avoid building x86_64-only binaries
for bin in "${BINARIES[@]}"; do
    echo "  Building $bin..."
    FEATURES=()
    for runtime_bin in "${RUNTIME_BINS[@]}"; do
        if [ "$bin" = "$runtime_bin" ]; then
            FEATURES=(--features runtime)
            break
        fi
    done
    if ! cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc "${FEATURES[@]}" --bin "$bin" 2>&1 | grep -E "^error" | head -3; then
        : # Success, no error output
    fi
done

echo ""
echo "Creating ELF files..."

for bin in "${BINARIES[@]}"; do
    if [ -f "target/aarch64-breenix/release/$bin" ]; then
        cp "target/aarch64-breenix/release/$bin" "aarch64/$bin.elf"
        echo "  - aarch64/$bin.elf"
    else
        echo "  WARNING: $bin not built (may have x86_64 dependencies)"
    fi
done

echo ""
echo "========================================"
echo "  ARM64 BUILD COMPLETE"
echo "========================================"
