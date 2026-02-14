#!/bin/bash
# Breenix Forensic Capture
# ========================
# Captures diagnostic state from a running (or deadlocked) Breenix QEMU instance.
#
# This script:
#   1. Connects to the QMP socket to pause the VM
#   2. Dumps guest memory via QMP (ELF core with per-CPU registers)
#   3. Attaches GDB to capture per-CPU backtraces and trace counters
#   4. Optionally resumes or kills the VM
#
# Usage:
#   scripts/forensic-capture.sh              # Capture and pause
#   scripts/forensic-capture.sh --resume     # Capture then resume VM
#   scripts/forensic-capture.sh --kill       # Capture then kill VM
#   scripts/forensic-capture.sh --timeout 30 # Auto-capture after 30s of no output
#
# Prerequisites:
#   - QEMU running with QMP socket (run.sh enables this by default)
#   - GDB (for backtraces): aarch64-none-elf-gdb or gdb-multiarch
#   - socat (for QMP communication)
#
# Output:
#   /tmp/breenix-forensic-<timestamp>/
#     ├── guest-memory.elf    # Full memory dump (loadable in GDB)
#     ├── backtraces.txt      # Per-CPU backtraces from GDB
#     ├── trace-counters.txt  # Kernel trace counter values
#     ├── qmp-status.json     # VM status at capture time
#     └── capture.log         # Script log

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(dirname "$SCRIPT_DIR")"

# Configuration
QMP_SOCK="${BREENIX_QMP_SOCK:-/tmp/breenix-qmp.sock}"
GDB_PORT="${BREENIX_GDB_PORT:-1234}"
ACTION="pause"  # pause, resume, kill

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --resume)
            ACTION="resume"
            shift
            ;;
        --kill)
            ACTION="kill"
            shift
            ;;
        --qmp)
            shift
            QMP_SOCK="$1"
            shift
            ;;
        --gdb-port)
            shift
            GDB_PORT="$1"
            shift
            ;;
        -h|--help)
            echo "Usage: scripts/forensic-capture.sh [OPTIONS]"
            echo ""
            echo "Captures diagnostic state from a running/deadlocked Breenix QEMU instance."
            echo ""
            echo "Options:"
            echo "  --resume       Resume VM after capture"
            echo "  --kill         Kill VM after capture"
            echo "  --qmp SOCK     QMP socket path (default: /tmp/breenix-qmp.sock)"
            echo "  --gdb-port N   GDB port (default: 1234)"
            echo "  -h, --help     Show this help"
            echo ""
            echo "Output: /tmp/breenix-forensic-<timestamp>/"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Create output directory
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
OUTPUT_DIR="/tmp/breenix-forensic-${TIMESTAMP}"
mkdir -p "$OUTPUT_DIR"
LOG="$OUTPUT_DIR/capture.log"

log() {
    echo "[$(date +%H:%M:%S)] $*" | tee -a "$LOG"
}

log "Breenix Forensic Capture"
log "========================"
log "QMP socket: $QMP_SOCK"
log "GDB port: $GDB_PORT"
log "Output: $OUTPUT_DIR"
log ""

# ─── Check prerequisites ─────────────────────────────────────────────────────

if ! command -v socat &>/dev/null; then
    log "ERROR: socat not found. Install with: brew install socat"
    exit 1
fi

if [ ! -S "$QMP_SOCK" ]; then
    log "ERROR: QMP socket not found at $QMP_SOCK"
    log "Make sure QEMU is running (via run.sh which enables QMP by default)"
    exit 1
fi

# ─── QMP helper ──────────────────────────────────────────────────────────────

# Send a QMP command and capture the response.
# QMP requires: 1) Read greeting, 2) Send qmp_capabilities, 3) Send command
qmp_command() {
    local cmd="$1"
    local output_file="${2:-/dev/null}"

    # Full QMP session: greeting → capabilities → command
    {
        # Wait for greeting, then negotiate capabilities, then send command
        sleep 0.2
        echo '{"execute": "qmp_capabilities"}'
        sleep 0.2
        echo "$cmd"
        sleep 0.5
    } | socat - UNIX-CONNECT:"$QMP_SOCK" 2>/dev/null | tail -1 > "$output_file"
}

# Send multiple QMP commands in one session
qmp_session() {
    local cmds=("$@")
    {
        sleep 0.2
        echo '{"execute": "qmp_capabilities"}'
        sleep 0.2
        for cmd in "${cmds[@]}"; do
            echo "$cmd"
            sleep 0.3
        done
        sleep 0.5
    } | socat - UNIX-CONNECT:"$QMP_SOCK" 2>/dev/null
}

# ─── Step 1: Pause VM ────────────────────────────────────────────────────────

log "Step 1: Pausing VM..."
qmp_command '{"execute": "stop"}' "$OUTPUT_DIR/qmp-stop.json"
log "  VM paused"

# ─── Step 2: Get VM status ───────────────────────────────────────────────────

log "Step 2: Capturing VM status..."
qmp_command '{"execute": "query-status"}' "$OUTPUT_DIR/qmp-status.json"
log "  Status saved to qmp-status.json"

# ─── Step 3: Dump guest memory ───────────────────────────────────────────────

MEMDUMP="$OUTPUT_DIR/guest-memory.elf"
log "Step 3: Dumping guest memory to ELF core..."
log "  (This produces a ~512MB file — includes per-CPU register state)"

# dump-guest-memory produces an ELF core file with NT_PRSTATUS notes
# containing per-CPU register state. This is loadable in GDB.
qmp_command "{\"execute\": \"dump-guest-memory\", \"arguments\": {\"paging\": false, \"protocol\": \"file:${MEMDUMP}\"}}" "$OUTPUT_DIR/qmp-dump.json"

# Wait for dump to complete (it's async)
for i in $(seq 1 60); do
    if [ -f "$MEMDUMP" ] && [ "$(stat -f%z "$MEMDUMP" 2>/dev/null || stat -c%s "$MEMDUMP" 2>/dev/null)" -gt 0 ]; then
        # Check if file is still growing
        local_size=$(stat -f%z "$MEMDUMP" 2>/dev/null || stat -c%s "$MEMDUMP" 2>/dev/null)
        sleep 1
        new_size=$(stat -f%z "$MEMDUMP" 2>/dev/null || stat -c%s "$MEMDUMP" 2>/dev/null)
        if [ "$local_size" = "$new_size" ]; then
            break
        fi
    fi
    sleep 1
done

if [ -f "$MEMDUMP" ]; then
    DUMP_SIZE=$(du -h "$MEMDUMP" | cut -f1)
    log "  Memory dump: $MEMDUMP ($DUMP_SIZE)"
else
    log "  WARNING: Memory dump may not have completed"
fi

# ─── Step 4: GDB backtraces ──────────────────────────────────────────────────

log "Step 4: Capturing GDB backtraces..."

# Detect architecture from kernel binary
KERNEL_ARCH=""
KERNEL_BIN=""
if [ -f "$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64" ]; then
    KERNEL_BIN="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
    KERNEL_ARCH="aarch64"
elif [ -f "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img ]; then
    KERNEL_BIN=$(ls -t "$BREENIX_ROOT/target/release/qemu-uefi" 2>/dev/null | head -1)
    KERNEL_ARCH="x86_64"
fi

# Find the right GDB binary for the target architecture
GDB_BIN=""
if [ "$KERNEL_ARCH" = "aarch64" ]; then
    # Prefer architecture-specific GDB for ARM64
    for candidate in aarch64-none-elf-gdb aarch64-elf-gdb aarch64-linux-gnu-gdb gdb-multiarch; do
        if command -v "$candidate" &>/dev/null; then
            GDB_BIN="$candidate"
            break
        fi
    done
    # Fall back to system gdb but warn about architecture mismatch
    if [ -z "$GDB_BIN" ] && command -v gdb &>/dev/null; then
        GDB_BIN="gdb"
        log "  WARNING: System gdb may not support aarch64. Install gdb-multiarch for proper backtraces."
    fi
else
    # x86_64 - system gdb works fine
    if command -v gdb &>/dev/null; then
        GDB_BIN="gdb"
    fi
fi

# Check if GDB port is open (VM was started with --debug)
GDB_AVAILABLE=false
if [ -n "$GDB_BIN" ]; then
    if nc -z localhost "$GDB_PORT" 2>/dev/null; then
        GDB_AVAILABLE=true
    else
        log "  GDB port $GDB_PORT not open. Start QEMU with --debug to enable GDB."
        log "  Skipping GDB backtraces (memory dump still available for offline analysis)."
    fi
else
    log "  GDB not found. Install aarch64-none-elf-gdb or gdb-multiarch."
    log "  Skipping GDB backtraces."
fi

if [ "$GDB_AVAILABLE" = true ] && [ -n "$KERNEL_BIN" ]; then
    log "  Using GDB: $GDB_BIN"
    log "  Kernel symbols: $KERNEL_BIN"

    # Create GDB command script
    GDB_CMDS="$OUTPUT_DIR/gdb-commands.txt"

    # Architecture-specific GDB setup
    if [ "$KERNEL_ARCH" = "aarch64" ]; then
        GDB_ARCH_SETUP="set architecture aarch64"
    else
        GDB_ARCH_SETUP="set architecture i386:x86-64"
    fi

    cat > "$GDB_CMDS" << GDBEOF
set pagination off
set confirm off
set print pretty on
$GDB_ARCH_SETUP

# Connect to QEMU
target remote :$GDB_PORT

# Per-CPU backtraces
echo \n=== PER-CPU BACKTRACES ===\n
thread apply all bt 20

# Register state for all CPUs
echo \n=== PER-CPU REGISTERS ===\n
thread apply all info registers

# Try to read trace counters via GDB
echo \n=== TRACE COUNTERS ===\n

# SYSCALL_TOTAL (per-CPU counter, slot 0 value at offset 8)
echo SYSCALL_TOTAL:
print SYSCALL_TOTAL.per_cpu[0].value
print SYSCALL_TOTAL.per_cpu[1].value
print SYSCALL_TOTAL.per_cpu[2].value
print SYSCALL_TOTAL.per_cpu[3].value

echo IRQ_TOTAL:
print IRQ_TOTAL.per_cpu[0].value
print IRQ_TOTAL.per_cpu[1].value
print IRQ_TOTAL.per_cpu[2].value
print IRQ_TOTAL.per_cpu[3].value

echo CTX_SWITCH_TOTAL:
print CTX_SWITCH_TOTAL.per_cpu[0].value
print CTX_SWITCH_TOTAL.per_cpu[1].value
print CTX_SWITCH_TOTAL.per_cpu[2].value
print CTX_SWITCH_TOTAL.per_cpu[3].value

echo TIMER_TICK_TOTAL:
print TIMER_TICK_TOTAL.per_cpu[0].value
print TIMER_TICK_TOTAL.per_cpu[1].value
print TIMER_TICK_TOTAL.per_cpu[2].value
print TIMER_TICK_TOTAL.per_cpu[3].value

# Scheduler state
echo \n=== SCHEDULER STATE ===\n
echo Global tick count:
print kernel::time::timer::TICKS

# Dump latest trace events if available
echo \n=== TRACE DUMP ===\n
call trace_dump_latest(50)
call trace_dump_counters()

# Disconnect cleanly
disconnect
quit
GDBEOF

    # Run GDB with timeout
    # --nx skips .gdbinit (which may load x86-specific config)
    timeout 30 "$GDB_BIN" --nx -batch -x "$GDB_CMDS" "$KERNEL_BIN" \
        > "$OUTPUT_DIR/backtraces.txt" 2>&1 || true

    if [ -f "$OUTPUT_DIR/backtraces.txt" ] && [ -s "$OUTPUT_DIR/backtraces.txt" ]; then
        log "  Backtraces saved to backtraces.txt"

        # Extract trace counters to separate file for easy viewing
        grep -A1 -E "(SYSCALL_TOTAL|IRQ_TOTAL|CTX_SWITCH_TOTAL|TIMER_TICK_TOTAL|FORK_TOTAL|EXEC_TOTAL)" \
            "$OUTPUT_DIR/backtraces.txt" > "$OUTPUT_DIR/trace-counters.txt" 2>/dev/null || true
        log "  Trace counters saved to trace-counters.txt"
    else
        log "  WARNING: GDB capture may have failed (check backtraces.txt)"
    fi
else
    if [ -z "$KERNEL_BIN" ]; then
        log "  Kernel binary not found for symbol loading."
    fi
    log "  NOTE: You can analyze the memory dump offline with:"
    log "    gdb -ex 'target core $MEMDUMP' $KERNEL_BIN"
fi

# ─── Step 5: Post-capture action ─────────────────────────────────────────────

log ""
case "$ACTION" in
    resume)
        log "Step 5: Resuming VM..."
        qmp_command '{"execute": "cont"}' /dev/null
        log "  VM resumed"
        ;;
    kill)
        log "Step 5: Killing VM..."
        qmp_command '{"execute": "quit"}' /dev/null
        log "  VM terminated"
        ;;
    pause)
        log "Step 5: VM remains paused."
        log "  To resume: echo '{\"execute\":\"cont\"}' | socat - UNIX-CONNECT:$QMP_SOCK"
        log "  To kill:   echo '{\"execute\":\"quit\"}' | socat - UNIX-CONNECT:$QMP_SOCK"
        ;;
esac

# ─── Summary ─────────────────────────────────────────────────────────────────

log ""
log "========================================"
log "  Forensic Capture Complete"
log "========================================"
log ""
log "Output directory: $OUTPUT_DIR"
log ""
log "Files:"
ls -lh "$OUTPUT_DIR" 2>/dev/null | while read -r line; do
    log "  $line"
done
log ""
log "Analyze with GDB:"
if [ -n "$KERNEL_BIN" ]; then
    log "  $GDB_BIN $KERNEL_BIN -ex 'target core $MEMDUMP'"
else
    log "  gdb <kernel-binary> -ex 'target core $MEMDUMP'"
fi
log ""
log "Quick check of per-CPU registers in the core dump:"
log "  readelf -n $MEMDUMP | head -50"
