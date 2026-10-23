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
# Environment:
#   BREENIX_RUST_FORK_LIBRARY  Override the forked Rust library path
#                              (default: <repo>/rust-fork/library)
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

# LIBC_RELEASE_DIR and RELEASE_DIR name each workspace's own target directory. An
# inherited target directory would send fresh binaries elsewhere while the
# installer below copied the old ones.
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR

# Gate cache builds use a stable source name across private lane trees/Cargo homes.
REMAP_FLAGS=""
if [ "${BREENIX_USERSPACE_REMAP:-0}" = 1 ]; then
    REMAP_FLAGS="--remap-path-prefix=$PROJECT_ROOT=/breenix --remap-path-prefix=$CARGO_HOME=/cargo"
fi

# Rustflags for linking against libbreenix-libc
# These are passed explicitly because cargo's [target.xxx] config sections
# don't match reliably when --target is a relative path (the path prefix
# prevents matching against the normalized target name).
STD_RUSTFLAGS="$REMAP_FLAGS -L native=$LIBC_RELEASE_DIR -C link-arg=-T$SCRIPT_DIR/linker.ld -C link-arg=--allow-multiple-definition -C default-linker-libraries=no"

echo "========================================"
echo "  STD USERSPACE BUILD (Rust std library)"
echo "========================================"
echo "  Architecture: $ARCH"
echo ""

# Reject mixed or uncommitted std inputs before any compilation.
"$PROJECT_ROOT/scripts/rust-fork.sh" verify

# Step 1: Build libbreenix-libc (produces libc.a)
echo "[1/3] Building libbreenix-libc ($ARCH)..."
LIBC_DIR="$PROJECT_ROOT/libs/libbreenix-libc"

if [ ! -d "$LIBC_DIR" ]; then
    echo "  ERROR: libs/libbreenix-libc not found"
    exit 1
fi

# `set -o pipefail` inside the subshell: without it the pipeline's status is the
# `while` loop's, which is always 0, so a FAILED cargo build reported success and
# the previous ELFs stayed installed. That is how a stale binary rides a green
# gate -- caught while mutating the #568 oracle, where an uncompilable mutation
# silently reused the last good build and "passed".
if ! (
    set -o pipefail
    cd "$LIBC_DIR" && \
    CARGO_ENCODED_RUSTFLAGS= \
    RUSTFLAGS="$REMAP_FLAGS" \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
); then
    echo "  ERROR: libbreenix-libc build FAILED"
    exit 1
fi
# compiler_builtins mixes ABI helpers with foreign libc math. Strip the latter
# after every Cargo build, including cache hits, before any C or Rust link.
LIBC_TOOLS="$("$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" --tool-dir)"
python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$LIBC_TOOLS"
echo "  libbreenix-libc built successfully"

# C programs link libc.a. aarch64's serves C as it is; x86_64's Rust build is
# soft-float, so C gets a second build of the same crate with the SysV float ABI.
if [[ "$ARCH" == "x86_64" ]]; then
    if ! (
        set -o pipefail
        cd "$LIBC_DIR" && \
        CARGO_ENCODED_RUSTFLAGS= \
        RUSTFLAGS="$REMAP_FLAGS" \
        cargo build --release --target ../../x86_64-breenix-c.json 2>&1 | while read line; do
            echo "  $line"
        done
    ); then
        echo "  ERROR: libbreenix-libc C-ABI build FAILED"
        exit 1
    fi
    python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" \
        --archive "$LIBC_DIR/target/x86_64-breenix-c/release/libc.a" --tool-dir "$LIBC_TOOLS"
    echo "  libbreenix-libc C-ABI build (x86_64-breenix-c) built successfully"
fi
python3 "$PROJECT_ROOT/scripts/libc-symbol-audit.py" --arch "$ARCH"
# The std link takes the first of two definitions (--allow-multiple-definition), so
# refuse a libc.a that defines a symbol twice.
python3 "$PROJECT_ROOT/scripts/libc-duplicate-check.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$LIBC_TOOLS"
echo ""

# Step 1b: C programs built against Breenix's own C library
echo "[1b/3] Building the libc suite's C programs ($ARCH)..."
if ! "$PROJECT_ROOT/userspace/libc-suite/build.sh" --arch "$ARCH"; then
    echo "  ERROR: libc suite C build FAILED"
    exit 1
fi
if ! "$PROJECT_ROOT/scripts/build-libc-busybox.sh" --arch "$ARCH"; then
    echo "  ERROR: BusyBox build against Breenix's libc FAILED"
    exit 1
fi
echo ""

# Additional POSIX C utilities are built only with Breenix's C driver.
UTILITY_BUILD_DIR="$SCRIPT_DIR/target/utilities-$ARCH"
mkdir -p "$TESTS_DIR" "$UTILITY_BUILD_DIR"
UTILITY_NAMES=" "
for source in "$PROJECT_ROOT/userspace/utilities"/*.c; do
    [ -f "$source" ] || continue
    name="$(basename "$source" .c)"
    if [ "$name" = libc-busybox ] || [ "$name" = busybox ] ||
        grep -Fqx "$name" "$TESTS_DIR/libc-busybox.links" ||
        awk -v name="$name" '
            $1 !~ /^#/ && name == "libc-" $1 "_test" { found=1 }
            END { exit !found }
        ' "$PROJECT_ROOT/userspace/libc-suite/categories"; then
        echo "ERROR: utility output name collision: $name" >&2
        exit 1
    fi
    echo "  Building utility $name ($ARCH)"
    "$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" -std=c17 -O2 -Wall -Wextra -Werror \
        "$source" -o "$UTILITY_BUILD_DIR/$name.elf"
    UTILITY_NAMES+="$name "
done

# B42 keeps the v1 stress subject only on x86-64.
LEGACY_GRAPHICS_FEATURES=()
if [[ "$ARCH" == "x86_64" ]]; then
    LEGACY_GRAPHICS_FEATURES=(--features framebuffer-v1)
fi

# Step 2: Build all userspace binaries
echo "[2/3] Building userspace ($ARCH)..."

RUST_FORK_LIBRARY="${BREENIX_RUST_FORK_LIBRARY:-$PROJECT_ROOT/rust-fork/library}"
if [ ! -d "$RUST_FORK_LIBRARY" ]; then
    echo "  ERROR: forked Rust library not found at $RUST_FORK_LIBRARY"
    echo "  Set BREENIX_RUST_FORK_LIBRARY to the forked Rust library path"
    echo "  The forked Rust compiler is required for std support"
    exit 1
fi

# Cargo's build-std freshness checks can miss edits in an external sysroot.
# Include its verified commit in compiler metadata so a pin change rebuilds std.
if [ -f "$RUST_FORK_LIBRARY/../.rust-fork-snapshot.json" ]; then
    FORK_COMMIT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$RUST_FORK_LIBRARY/../.rust-fork-snapshot.json")"
else
    FORK_COMMIT="$(git -C "$RUST_FORK_LIBRARY/.." rev-parse HEAD)"
fi
STD_RUSTFLAGS+=" -C metadata=breenix_std_$FORK_COMMIT"

if ! (
    set -o pipefail
    cd "$SCRIPT_DIR" && \
    unset CARGO_ENCODED_RUSTFLAGS && \
    __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY" \
    RUSTFLAGS="$STD_RUSTFLAGS" \
    cargo build --release --target "$TARGET_JSON" ${LEGACY_GRAPHICS_FEATURES[@]+"${LEGACY_GRAPHICS_FEATURES[@]}"} 2>&1 | while read line; do
        echo "  $line"
    done
); then
    echo "  ERROR: userspace build FAILED"
    exit 1
fi

echo "  Userspace build successful"
# And refuse a C symbol that libc.a and another input of the std link both define:
# the std programs would link whichever came first.
STD_LINK_INPUTS=()
for rlib in "$SCRIPT_DIR/target/$TARGET_DIR/release/deps/"*.rlib; do
    STD_LINK_INPUTS+=(--link-input "$rlib")
done
# The rlibs' objects are LLVM bitcode from rustc's own LLVM, which an older system
# llvm-nm cannot read; the pinned toolchain's llvm-tools always can.
RUST_TOOLS="$(cd "$PROJECT_ROOT" && rustc --print sysroot)/lib/rustlib/$(cd "$PROJECT_ROOT" && rustc -vV | sed -n 's/^host: //p')/bin"
python3 "$PROJECT_ROOT/scripts/libc-duplicate-check.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$RUST_TOOLS" "${STD_LINK_INPUTS[@]}"
echo ""

# Step 3: Copy all binaries as .elf to userspace/programs/ for ext2 inclusion
echo "[3/3] Installing std binaries..."

# All std binaries to install
# Format: "binary_name:elf_name" (elf_name defaults to binary_name.elf)
STD_BINARIES=(
    "gpu_fault_acceptance"
    "fifo_fs_test"
    # hello_std_real replaces hello_world on disk
    "hello_std_real:hello_world"

    # Exec smoke (aarch64 boot-path execve coverage)
    "exec_smoke"
    "exec_smoke_target"
    "umask_exec_test"

    # Fork smoke (#745 -- x86 production fork() coverage, arch-neutral)
    "fork_smoke"

    # #1321: a thread outlives its process's first thread
    "thread_teardown_test"

    # #575 block-EINTR oracle
    "block_eintr_oracle"
    # #568 blocking-poll-on-connected-TCP oracle
    "poll_tcp_oracle"
    # #584 futex handoff oracle
    "futex_handoff_oracle"
    # Green-program arc 4: production-profile TTY/PTY evidence leg
    "tty_oracle"
    # #737 direction-flag preempt oracle (x86 only; fork-free)
    "df_preempt_oracle"

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
    "pipe_fifo_blocking_oracle"
    "pipe_fifo_blocking_supervisor"
    "unix_stream_blocking_oracle"
    "unix_stream_blocking_supervisor"
    "dup_test"
    "fcntl_test"
    "poll_test"
    "select_test"
    "epoll_test"
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
    "sigkill_teardown_test"
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

    # Phase 3: Coreutils now provided by BusyBox (see scripts/build-libc-busybox.sh)
    "resolution"

    # Phase 4: init_shell
    "init_shell"

    # Phase 5: Newly ported programs

    # Simple tests + examples
    "argv_test"
    "job_table_test"
    "test_mmap"
    "clonevm_exec_test"
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
    "simple_exit0"
    "spawn_smoke_target"
    "counter"
    "spinner"
    "hello_time"
    "heartbeat"
    "xhci_counters"
    "fbinfo_test"
    "bounce"
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
    "tcp_dup_listener_test"
    "tcp_cloexec_exec_test"
    "tcp_blocking_test"
    "concurrent_recv_stress"
    "dns_test"
    "net_test"
    "http_fetch_test"
    "loopback_wake_test"

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
    "getty"
    "chvt"
    "login"
    "probe"
    "persistent-install"
    "sync"

    # Effort suites, installed as /sbin/suite-<id>
    "suite-smoke"
    "suite-crypto"
    "suite-files-io"
    "suite-directories"
    "suite-processes"
    "suite-signals"
    "suite-time"
    "suite-memory"
    "suite-threads"
    "suite-ipc"
    "suite-terminals"
    "suite-networking"
    "suite-rust-std"
    "suite-libc"
    "suite-utilities"
    "suite-sdk"
    "suite-breengel"
    "kiosk_test"
    "raw-window_test"
    "suite-gpu"
    "gpu-exec-probe"
    "suite-console"
    "console-login-shell"
    "files-io-exec_test"
    "processes-exec_test"
    "rust-std-child_test"
    "signals-exec_test"
    "time-exec_test"
    "telnetd"
    "blogd"

    # Diagnostics
    "btrace"
    "usb_pointer_check"

    # Log viewer
    "bless"

    # Self-check test runner
    "bcheck"

    # Apps
    "bterm"
    "blog"
    "bfontpicker"
    "blauncher"
    "bsshd"
    "bssh"
)

if [[ "$ARCH" == "x86_64" ]]; then
    STD_BINARIES+=("wait_stress")
else
    rm -f "$TESTS_DIR/wait_stress.elf"
fi

# A retained target may contain an executable whose Cargo target was removed.
# Validate against this invocation's manifest, not the persistent output pool.
CURRENT_TARGETS="$(cd "$SCRIPT_DIR" && cargo metadata --no-deps --format-version 1)"
printf '%s' "$CURRENT_TARGETS" | python3 -c '
import json, os, sys
metadata = json.load(sys.stdin)
manifest = os.path.realpath(sys.argv[1])
names = {target["name"] for package in metadata["packages"]
         if os.path.realpath(package["manifest_path"]) == manifest
         for target in package["targets"] if "bin" in target["kind"]}
missing = [entry.split(":", 1)[0] for entry in sys.argv[2:]
           if entry.split(":", 1)[0] not in names]
if missing:
    sys.exit("ERROR: install list has no current Cargo binary target: " + ", ".join(missing))
' "$SCRIPT_DIR/Cargo.toml" "${STD_BINARIES[@]}"

# Check producer names before copying outputs into the shared ELF directory.
SEEN_NAMES="$UTILITY_NAMES libc-busybox "
while read -r category _; do
    case "$category" in ''|'#'*) continue ;; esac
    SEEN_NAMES+="libc-${category}_test "
done < "$PROJECT_ROOT/userspace/libc-suite/categories"
for entry in "${STD_BINARIES[@]}"; do
    name="${entry##*:}"
    if [[ "$SEEN_NAMES" == *" $name "* ]] || [ "$name" = busybox ] ||
        grep -Fqx "$name" "$TESTS_DIR/libc-busybox.links"; then
        echo "ERROR: ELF output name collision: $name" >&2
        exit 1
    fi
    SEEN_NAMES+="$name "
done

for name in $UTILITY_NAMES; do
    cp "$UTILITY_BUILD_DIR/$name.elf" "$TESTS_DIR/$name.elf"
done

RELEASE_DIR="$SCRIPT_DIR/target/$TARGET_DIR/release"
INSTALLED=0
FAILED=0

mkdir -p "$TESTS_DIR"

# Remove stale ELFs absent from the current producer list before image packing.
INSTALL_NAMES="$UTILITY_NAMES"
for entry in "${STD_BINARIES[@]}"; do
    INSTALL_NAMES+="${entry##*:} "
done
# The C programs userspace/libc-suite/build.sh and scripts/build-libc-busybox.sh
# installed above.
INSTALL_NAMES+="libc-busybox "
while read -r category _; do
    case "$category" in ''|'#'*) continue ;; esac
    INSTALL_NAMES+="libc-${category}_test "
done < "$PROJECT_ROOT/userspace/libc-suite/categories"
for elf in "$TESTS_DIR"/*.elf; do
    [ -e "$elf" ] || continue
    name="$(basename "$elf" .elf)"
    if [[ "$INSTALL_NAMES" != *" $name "* ]]; then
        rm -f "$elf"
        echo "  Removed stale ${name}.elf"
    fi
done

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
    exit 1
fi
echo "========================================"
# Prebuilt consumers verify the installed ELFs against this record.
python3 "$PROJECT_ROOT/scripts/userspace-manifest.py" write --arch "$ARCH"
