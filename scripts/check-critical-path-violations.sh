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

    # BXCAP failure-trace capture. A DIRECTORY entry (trailing slash): the
    # `.rs` files under kernel/src/capture/ are enumerated from disk and
    # checked -- 4 of 4 today -- so a file added to the module is covered the
    # day it is added rather than the day someone remembers to list it here.
    # The emitter runs from fault handlers and masked interrupt context, so
    # it carries the extra capture-scoped denylist below on top of the shared
    # one.
    "capture/"
)

# Additional patterns prohibited ONLY under kernel/src/capture/.
#
# These are not in the shared list above because the shared list applies to
# files that legitimately contain them: task/scheduler.rs takes locks and
# allocates by its nature, and flagging it for that would make this script
# useless. The capture path is different -- it is entered from a fault
# handler or a masked interrupt, where a blocking lock deadlocks and an
# allocation takes the heap lock from interrupt context.
#
#   \.lock()        a BLOCKING lock acquisition. `try_lock()` does not match
#                   this pattern (there is no `.` before `lock` in
#                   `.try_lock()`), which is the distinction: the capture may
#                   ASK for a lock, it may not WAIT for one.
#   try_dump_state  the scheduler's allocating dump. It builds two `alloc`
#                   vectors while holding the guard; `try_liveness_snapshot`
#                   is the fixed-size, allocation-free sibling the capture
#                   uses instead. Named explicitly because it is the one
#                   wrong turn that LOOKS non-blocking.
#   alloc::, Vec, String, Box, vec!, to_string
#                   heap allocation in any spelling this tree uses.
#   unwrap(), expect(, panic!
#                   a panic from inside a capture re-enters the panic path
#                   the capture is meant to report from.
CAPTURE_PROHIBITED_PATTERNS=(
    '\.lock()'
    'try_dump_state'
    'alloc::'
    'Vec<'
    'String'
    'Box<'
    'vec!'
    'to_string'
    'unwrap()'
    'expect('
    'panic!'
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
    # serial_print!/log_serial_print! are the non-`ln` siblings of
    # serial_println!/log_serial_println! -- same blocking lock, no
    # newline, so they escaped this list until the census in
    # tests/critical_path_logging_census_structure.rs found the one live
    # site (arch_impl/aarch64/exception.rs :: fn sys_write, a per-byte
    # `crate::serial_print!` in an EL1-only fallback). log::log! is the
    # fourth way to reach CombinedLogger::log alongside
    # log::{debug,info,warn,error}! (already denied above) and log::trace!
    # (already denied, though it emits 0 bytes today).
    'serial_print!'
    'log_serial_print!'
    'log::log!'
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

    # The capture path carries the shared list plus its own; the other
    # critical files carry the shared list alone.
    local patterns=("${PROHIBITED_PATTERNS[@]}")
    case "$relative_path" in
        capture/*|*/capture/*)
            patterns+=("${CAPTURE_PROHIBITED_PATTERNS[@]}")
            ;;
    esac

    for pattern in "${patterns[@]}"; do
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

        # A directory entry expands to the .rs files beneath it. An entry
        # that expands to 0 files is an ERROR, not a quiet pass: a renamed
        # or deleted directory must not read as "no violations here".
        if [[ "$critical_file" == */ ]]; then
            local matched=0
            local member
            while IFS= read -r member; do
                [[ -n "$member" ]] || continue
                matched=$((matched + 1))
                check_file "$member"
            done < <(find "$full_path" -type f -name '*.rs' 2>/dev/null | sort)
            if [[ $matched -eq 0 ]]; then
                echo -e "${RED}ERROR${NC}: critical directory ${YELLOW}$critical_file${NC} matched no .rs file."
                echo "This script checked nothing there. Fix the entry rather than deleting it."
                found_violations=1
            else
                echo "  $critical_file: $matched file(s) checked"
            fi
            continue
        fi

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
