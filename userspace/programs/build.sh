#!/bin/bash
# Build Rust std test binaries for Breenix
#
# This script builds all std-based test binaries
# and copies them to the ext2 disk image.
#
# Dependencies:
#   - rust-fork/library (forked Rust std with target_os = "breenix")
#   - libs/libbreenix-libc (provides libc.a for std's Unix PAL)
#
# Usage:
#   ./userspace/programs/build.sh                  # x86_64 (default)
#   ./userspace/programs/build.sh --arch aarch64   # aarch64

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Default architecture
ARCH="x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--arch x86_64|aarch64]"
            exit 1
            ;;
    esac
done

# Set architecture-specific variables
if [[ "$ARCH" == "aarch64" ]]; then
    TARGET_JSON="../../aarch64-breenix.json"
    TARGET_DIR="aarch64-breenix"
    TESTS_DIR="$PROJECT_ROOT/userspace/programs/aarch64"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/aarch64-breenix/release"
else
    TARGET_JSON="../../x86_64-breenix.json"
    TARGET_DIR="x86_64-breenix"
    TESTS_DIR="$SCRIPT_DIR"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/x86_64-breenix/release"
fi

# Rustflags for linking against libbreenix-libc
# These are passed explicitly because cargo's [target.xxx] config sections
# don't match reliably when --target is a relative path (the path prefix
# prevents matching against the normalized target name).
STD_RUSTFLAGS="-L native=$LIBC_RELEASE_DIR -C link-arg=-T$SCRIPT_DIR/linker.ld -C link-arg=--allow-multiple-definition -C default-linker-libraries=no"

echo "========================================"
echo "  STD USERSPACE BUILD (Rust std library)"
echo "========================================"
echo "  Architecture: $ARCH"
echo ""

# Step 1: Build libbreenix-libc (produces libc.a)
echo "[1/3] Building libbreenix-libc ($ARCH)..."
LIBC_DIR="$PROJECT_ROOT/libs/libbreenix-libc"

if [ ! -d "$LIBC_DIR" ]; then
    echo "  ERROR: libs/libbreenix-libc not found"
    exit 1
fi

(cd "$LIBC_DIR" && \
    CARGO_ENCODED_RUSTFLAGS= \
    RUSTFLAGS= \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
)
echo "  libbreenix-libc built successfully"
echo ""

# Step 2: Build all userspace binaries
echo "[2/3] Building userspace ($ARCH)..."

RUST_FORK_LIBRARY="$PROJECT_ROOT/rust-fork/library"
if [ ! -d "$RUST_FORK_LIBRARY" ]; then
    echo "  ERROR: rust-fork/library not found"
    echo "  The forked Rust compiler is required for std support"
    exit 1
fi

(cd "$SCRIPT_DIR" && \
    unset CARGO_ENCODED_RUSTFLAGS && \
    __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY" \
    RUSTFLAGS="$STD_RUSTFLAGS" \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
)

echo "  Userspace build successful"
echo ""

# Step 3: Copy all binaries as .elf to userspace/programs/ for ext2 inclusion
echo "[3/3] Installing std binaries..."

# All std binaries to install
# Format: "binary_name:elf_name" (elf_name defaults to binary_name.elf)
STD_BINARIES=(
    # hello_std_real replaces hello_world on disk
    "hello_std_real:hello_world"

    # Phase 1: No-fork programs
    "syscall_enosys"
    "clock_gettime_test"
    "file_read_test"
    "lseek_test"
    "fs_write_test"
    "fs_rename_test"
    "fs_large_file_test"
    "fs_directory_test"
    "fs_link_test"
    "access_test"
    "devfs_test"
    "cwd_test"
    "getdents_test"
    "pipe_test"
    "pipe2_test"
    "dup_test"
    "fcntl_test"
    "poll_test"
    "select_test"
    "nonblock_test"
    "brk_test"
    "signal_handler_test"
    "signal_return_test"
    "signal_regs_test"
    "sigaltstack_test"
    "sigsuspend_test"
    "pause_test"
    "tty_test"
    "session_test"
    "unix_socket_test"
    "unix_named_socket_test"
    "fifo_test"

    # Phase 2: Fork-dependent programs
    "fork_test"
    "fork_memory_test"
    "fork_state_test"
    "waitpid_test"
    "exec_argv_test"
    "cloexec_test"
    "kill_process_group_test"
    "sigchld_test"
    "sigchld_job_test"
    "ctrl_c_test"
    "job_control_test"
    "signal_fork_test"
    "signal_exec_test"
    "wnohang_timing_test"
    "fork_pending_signal_test"
    "shell_pipe_test"
    "pipeline_test"

    # Phase 2: CoW tests
    "cow_cleanup_test"
    "cow_sole_owner_test"
    "cow_stress_test"
    "cow_readonly_test"
    "cow_signal_test"

    # Phase 3: Coreutils (b-prefixed)
    "btrue"
    "bfalse"
    "becho"
    "bcat"
    "bhead"
    "btail"
    "bwc"
    "bwhich"
    "bls"
    "bmkdir"
    "brmdir"
    "brm"
    "bcp"
    "bmv"
    "resolution"

    # Phase 4: init_shell
    "init_shell"

    # Phase 5: Newly ported programs

    # Simple tests + examples
    "argv_test"
    "job_table_test"
    "test_mmap"
    "stdin_test"
    "true_test"
    "false_test"
    "echo_argv_test"
    "mkdir_argv_test"
    "rm_argv_test"
    "cp_mv_argv_test"
    "nonblock_eagain_test"
    "blocking_recv_test"
    "tcp_client_test"
    "simple_exit"
    "counter"
    "spinner"
    "hello_time"
    "fbinfo_test"
    "demo"
    "bounce"
    "particles"
    "confetti"
    "tones"
    "fart"
    "http_test"
    "register_init_test"

    # Coreutil tests
    "head_test"
    "tail_test"
    "wc_test"
    "which_test"
    "cat_test"
    "ls_test"
    "exec_stack_argv_test"
    "exec_from_ext2_test"
    "pipe_fork_test"
    "pipe_concurrent_test"
    "fs_block_alloc_test"
    "cow_oom_test"

    # Signal/timer tests
    "signal_test"
    "alarm_test"
    "itimer_test"
    "timer_test"
    "sleep_debug_test"
    "pipe_refcount_test"

    # Network tests
    "udp_socket_test"
    "tcp_socket_test"
    "tcp_blocking_test"
    "concurrent_recv_stress"
    "dns_test"

    # Complex/arch-specific tests
    "syscall_diagnostic_test"
    "pty_test"
    "signal_exec_check"

    # Shells
    "bsh"

    # Window manager and system monitor
    "bwm"
    "btop"

    # Network tools
    "burl"

    # Services
    "init"
    "telnetd"
)

RELEASE_DIR="$SCRIPT_DIR/target/$TARGET_DIR/release"
INSTALLED=0
FAILED=0

mkdir -p "$TESTS_DIR"

if [ -d "$TESTS_DIR" ]; then
    for entry in "${STD_BINARIES[@]}"; do
        # Parse "name:elf_name" or just "name"
        if [[ "$entry" == *":"* ]]; then
            BIN_NAME="${entry%%:*}"
            ELF_NAME="${entry##*:}"
        else
            BIN_NAME="$entry"
            ELF_NAME="$entry"
        fi

        SRC="$RELEASE_DIR/$BIN_NAME"
        DST="$TESTS_DIR/${ELF_NAME}.elf"

        if [ -f "$SRC" ]; then
            cp "$SRC" "$DST"
            SIZE=$(stat -f%z "$DST" 2>/dev/null || stat -c%s "$DST")
            echo "  Installed ${ELF_NAME}.elf ($SIZE bytes)"
            INSTALLED=$((INSTALLED + 1))
        else
            echo "  WARNING: $BIN_NAME not found at $SRC"
            FAILED=$((FAILED + 1))
        fi
    done
else
    echo "  WARNING: $TESTS_DIR not found, skipping ext2 copy"
fi

echo ""
echo "========================================"
echo "  STD BUILD COMPLETE ($ARCH)"
echo "  Installed: $INSTALLED binaries"
if [ $FAILED -gt 0 ]; then
    echo "  Failed: $FAILED binaries"
fi
echo "========================================"
