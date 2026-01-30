#!/bin/bash
#
# Check for logging violations in critical paths.
#
# This script scans kernel source files for prohibited logging patterns
# in files/functions that are on the critical path (interrupt handlers,
# context switch, syscall hot path, etc.).
#
# Exit code:
#   0 - No violations found
#   1 - Violations found
#
# Usage:
#   ./scripts/check-critical-path-violations.sh
#   ./scripts/check-critical-path-violations.sh path/to/file.rs  # Check specific file

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$SCRIPT_DIR/../kernel/src"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Critical path files - logging is NEVER allowed in these
CRITICAL_FILES=(
    # Context switch
    "arch_impl/aarch64/context_switch.rs"
    "arch_impl/aarch64/context.rs"
    "interrupts/context_switch.rs"

    # Interrupt handlers
    "arch_impl/aarch64/timer_interrupt.rs"
    "arch_impl/aarch64/exception.rs"
    "interrupts/timer.rs"
    "interrupts/timer_entry.asm"

    # Syscall hot path
    "syscall/handler.rs"
    "syscall/entry.asm"
    "syscall/time.rs"

    # Per-CPU data (accessed in hot paths)
    "per_cpu.rs"
    "per_cpu_aarch64.rs"
    "arch_impl/aarch64/percpu.rs"

    # Scheduler (called from context switch during timer interrupt)
    "task/scheduler.rs"
)

# Prohibited patterns - these use locks or formatting which is forbidden
PROHIBITED_PATTERNS=(
    'serial_println!'
    'log::debug!'
    'log::info!'
    'log::warn!'
    'log::error!'
    'log::trace!'
    'println!'
    'eprintln!'
    'format!'
    'write!'
    'writeln!'
    # These are the crate-level macros
    'crate::serial_println!'
)

# Allowed patterns (lock-free alternatives)
# These are explicitly ALLOWED in critical paths:
#   - raw_uart_char() / raw_uart_str() - lock-free UART output
#   - trace_event() / trace_*! macros - lock-free ring buffer
#   - raw_serial_char() / raw_serial_str() - lock-free serial output

found_violations=0

check_file() {
    local file="$1"
    local relative_path="${file#$KERNEL_DIR/}"

    # Skip if file doesn't exist
    if [[ ! -f "$file" ]]; then
        return
    fi

    local file_has_violations=0

    for pattern in "${PROHIBITED_PATTERNS[@]}"; do
        # Use grep to find matches, excluding comments
        # Note: This is a simple check - doesn't handle all edge cases
        if grep -n "$pattern" "$file" 2>/dev/null | grep -v "^[^:]*:[[:space:]]*//"; then
            if [[ $file_has_violations -eq 0 ]]; then
                echo -e "${RED}VIOLATION${NC} in ${YELLOW}$relative_path${NC}:"
                file_has_violations=1
            fi
            found_violations=1
        fi
    done
}

check_all_critical_files() {
    echo "Checking critical path files for logging violations..."
    echo ""

    for critical_file in "${CRITICAL_FILES[@]}"; do
        local full_path="$KERNEL_DIR/$critical_file"
        if [[ -f "$full_path" ]]; then
            check_file "$full_path"
        fi
    done
}

# Main entry point
if [[ $# -gt 0 ]]; then
    # Check specific file(s)
    for file in "$@"; do
        check_file "$file"
    done
else
    # Check all critical files
    check_all_critical_files
fi

echo ""
if [[ $found_violations -eq 0 ]]; then
    echo -e "${GREEN}No critical path violations found.${NC}"
    exit 0
else
    echo -e "${RED}Critical path violations detected!${NC}"
    echo ""
    echo "These files are on the kernel's critical path (interrupt handlers,"
    echo "context switch, syscall hot path). Logging using locks is FORBIDDEN."
    echo ""
    echo "Allowed alternatives:"
    echo "  - raw_uart_char(b'X')     - Single character, no locks"
    echo "  - raw_serial_char(0x41)   - Single character to serial"
    echo "  - trace_event(TYPE, val)  - Lock-free ring buffer trace"
    echo "  - trace_marker!(A)        - Lock-free debug marker"
    echo ""
    echo "See kernel/src/trace.rs for the lock-free tracing framework."
    exit 1
fi
