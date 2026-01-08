#!/bin/bash
set -e

# Add LLVM tools (rust-objcopy) to PATH
# llvm-tools-preview installs to the rustup toolchain's lib directory
SYSROOT=$(rustc --print sysroot)
HOST_TRIPLE=$(rustc -vV | grep host | cut -d' ' -f2)
LLVM_TOOLS_PATH="$SYSROOT/lib/rustlib/$HOST_TRIPLE/bin"
if [ -d "$LLVM_TOOLS_PATH" ]; then
    export PATH="$LLVM_TOOLS_PATH:$PATH"
fi

# Verify rust-objcopy is available
if ! command -v rust-objcopy &> /dev/null; then
    echo "ERROR: rust-objcopy not found"
    echo "Install llvm-tools-preview: rustup component add llvm-tools-preview"
    exit 1
fi

echo "========================================"
echo "  USERSPACE TEST BUILD (with libbreenix)"
echo "========================================"
echo ""

# Show the libbreenix dependency
echo "Dependency: libbreenix (syscall wrapper library)"
echo "  Location: ../../libs/libbreenix"
echo "  Provides: process, io, time, memory syscall wrappers"
echo ""

# List binaries being built
BINARIES=(
    "hello_world"
    "hello_time"
    "counter"
    "spinner"
    "fork_test"
    "timer_test"
    "syscall_enosys"
    "clock_gettime_test"
    "register_init_test"
    "syscall_diagnostic_test"
    "brk_test"
    "test_mmap"
    "signal_test"
    "signal_handler_test"
    "signal_return_test"
    "signal_regs_test"
    "udp_socket_test"
    "pipe_test"
    "pipe_fork_test"
    "pipe_concurrent_test"
    "pipe_refcount_test"
    "stdin_test"
    "init_shell"
    "waitpid_test"
    "signal_fork_test"
    "fork_pending_signal_test"
    "sigchld_test"
    "wnohang_timing_test"
    "signal_exec_test"
    "signal_exec_check"
    "pause_test"
    "dup_test"
    "fcntl_test"
    "pipe2_test"
    "cloexec_test"
    "poll_test"
    "select_test"
    "nonblock_test"
    "tty_test"
    "job_control_test"
    "session_test"
    "job_table_test"
    "pipeline_test"
    "sigchld_job_test"
    "file_read_test"
    "ctrl_c_test"
    "getdents_test"
    "lseek_test"
    "fs_write_test"
    "fs_rename_test"
    "fs_large_file_test"
    "fs_directory_test"
    "fs_link_test"
    "access_test"
    "fork_memory_test"
    "fork_state_test"
    "argv_test"
    "cat"
    "exec_argv_test"
    # Coreutils
    "ls_cmd"
    "echo_cmd"
    "mkdir_cmd"
    "rmdir_cmd"
    "rm_cmd"
    "cp_cmd"
    "mv_cmd"
)

echo "Building ${#BINARIES[@]} userspace binaries with libbreenix..."
echo ""

# Build with cargo (config is in .cargo/config.toml)
# This will compile libbreenix first, then link it into each binary
cargo build --release 2>&1 | while read line; do
    # Highlight libbreenix compilation
    if echo "$line" | grep -q "Compiling libbreenix"; then
        echo "  [libbreenix] $line"
    elif echo "$line" | grep -q "Compiling userspace_tests"; then
        echo "  [userspace]  $line"
    else
        echo "  $line"
    fi
done

echo ""
echo "Copying ELF binaries..."

# Copy and report each binary
for bin in "${BINARIES[@]}"; do
    cp "target/x86_64-breenix/release/$bin" "$bin.elf"
    echo "  - $bin.elf (uses libbreenix)"
done

echo ""
echo "Creating flat binaries..."

# Create flat binaries
for bin in "${BINARIES[@]}"; do
    rust-objcopy -O binary "$bin.elf" "$bin.bin"
done

echo ""
echo "========================================"
echo "  BUILD COMPLETE - libbreenix binaries"
echo "========================================"
echo ""
echo "Binary sizes:"
for bin in "${BINARIES[@]}"; do
    size=$(stat -f%z "$bin.bin" 2>/dev/null || stat -c%s "$bin.bin")
    printf "  %-30s %6d bytes\n" "$bin.bin" "$size"
done
echo ""
echo "These binaries use libbreenix for syscalls:"
echo "  - libbreenix::process (exit, fork, exec, getpid, gettid, yield)"
echo "  - libbreenix::io (read, write, print, println, close, pipe)"
echo "  - libbreenix::fs (open, read, fstat, lseek, close)"
echo "  - libbreenix::time (clock_gettime)"
echo "  - libbreenix::memory (brk, sbrk)"
echo "  - libbreenix::signal (kill, sigaction, sigprocmask)"
echo "========================================"
