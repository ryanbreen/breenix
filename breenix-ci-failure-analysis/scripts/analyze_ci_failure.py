#!/usr/bin/env python3
"""
Analyze CI failure logs to identify root causes.

This script helps diagnose common Breenix CI failures by parsing log files
and looking for known failure patterns.
"""

import argparse
import re
import sys
from pathlib import Path
from typing import List, Tuple, Optional


class FailurePattern:
    """Represents a known failure pattern with diagnosis info."""
    def __init__(self, name: str, pattern: str, diagnosis: str, fix: str, is_regex: bool = False):
        self.name = name
        self.pattern = pattern if not is_regex else re.compile(pattern)
        self.diagnosis = diagnosis
        self.fix = fix
        self.is_regex = is_regex

    def matches(self, line: str) -> bool:
        """Check if this pattern matches the line."""
        if self.is_regex:
            return self.pattern.search(line) is not None
        return self.pattern in line


# Known failure patterns
FAILURE_PATTERNS = [
    FailurePattern(
        "Double Fault",
        r"DOUBLE FAULT.*Error Code: (0x[0-9a-fA-F]+)",
        "Kernel encountered a double fault - usually indicates stack corruption, unmapped memory access during exception handling, or page table issues",
        "Check: 1) Kernel stack mapping in process page tables 2) IST stack configuration 3) Page table entry flags 4) Recent memory management changes",
        is_regex=True
    ),
    FailurePattern(
        "Page Fault",
        r"PAGE FAULT.*at (0x[0-9a-fA-F]+).*Error Code: (0x[0-9a-fA-F]+)",
        "Page fault accessing unmapped or incorrectly mapped memory",
        "Identify the faulting address and check: 1) Is it mapped in the active page table? 2) Are the flags correct (USER_ACCESSIBLE, WRITABLE)? 3) Was it recently unmapped?",
        is_regex=True
    ),
    FailurePattern(
        "Test Timeout",
        "Timeout",
        "Test exceeded time limit - could be kernel hang, infinite loop, or test too slow for CI",
        "Check: 1) Does test complete locally? 2) Are there any infinite loops? 3) Is timer interrupt working? 4) Increase timeout if legitimately slow",
        is_regex=False
    ),
    FailurePattern(
        "QEMU Not Found",
        "qemu-system-x86_64: command not found",
        "QEMU not installed in CI environment",
        "Add 'qemu-system-x86' to system dependencies in GitHub workflow",
        is_regex=False
    ),
    FailurePattern(
        "Rust Target Missing",
        "error: target 'x86_64-unknown-none' may not be installed",
        "Custom kernel target not available",
        "Add 'target: x86_64-unknown-none' to Rust toolchain installation step",
        is_regex=False
    ),
    FailurePattern(
        "rust-src Missing",
        "error: could not compile `bootloader`",
        "Missing rust-src component required for no_std builds",
        "Add 'rust-src' to components list in Rust toolchain setup",
        is_regex=False
    ),
    FailurePattern(
        "Userspace Binary Missing",
        r"Userspace binary not found|Error loading ELF",
        "Userspace test binary not built before kernel test",
        "Add userspace build step before kernel test: 'cd userspace/tests && ./build.sh'",
        is_regex=True
    ),
    FailurePattern(
        "Compilation Error",
        r"error(?:\[E\d+\])?:",
        "Rust compilation failed",
        "Check: 1) Correct Rust nightly version 2) All required features enabled 3) No syntax errors 4) Dependencies available",
        is_regex=True
    ),
    FailurePattern(
        "Signal Not Found",
        "no evidence of userspace execution",
        "Expected kernel log signal not found in output - test did not complete successfully",
        "Check: 1) Does kernel boot at all? 2) Does it reach the expected checkpoint? 3) Is the signal string correct? 4) Was test code executed?",
        is_regex=False
    ),
    FailurePattern(
        "Kernel Panic",
        "PANIC",
        "Kernel panic - unrecoverable error",
        "Read panic message for specific cause. Common: assertion failure, unwrap() on None, index out of bounds, explicit panic!()",
        is_regex=False
    ),
]


def find_patterns(log_content: str) -> List[Tuple[FailurePattern, str, int]]:
    """Find all matching failure patterns in the log."""
    matches = []
    lines = log_content.split('\n')

    for i, line in enumerate(lines, 1):
        for pattern in FAILURE_PATTERNS:
            if pattern.matches(line):
                matches.append((pattern, line, i))

    return matches


def extract_context(log_content: str, line_num: int, context: int = 5) -> str:
    """Extract lines around a specific line number."""
    lines = log_content.split('\n')
    start = max(0, line_num - context - 1)
    end = min(len(lines), line_num + context)

    context_lines = []
    for i in range(start, end):
        prefix = ">>> " if i == line_num - 1 else "    "
        context_lines.append(f"{prefix}{i+1:5d}: {lines[i]}")

    return '\n'.join(context_lines)


def analyze_log_file(log_file: Path, verbose: bool = False) -> dict:
    """Analyze a log file and return findings."""
    try:
        log_content = log_file.read_text()
    except Exception as e:
        return {"error": f"Failed to read log file: {e}"}

    matches = find_patterns(log_content)

    # Deduplicate by pattern name
    unique_patterns = {}
    for pattern, line, line_num in matches:
        if pattern.name not in unique_patterns:
            unique_patterns[pattern.name] = (pattern, line, line_num)

    return {
        "log_file": str(log_file),
        "total_lines": len(log_content.split('\n')),
        "patterns_found": len(unique_patterns),
        "matches": unique_patterns,
        "log_content": log_content if verbose else None,
    }


def print_analysis(analysis: dict, show_context: bool = False):
    """Print analysis results in a readable format."""
    if "error" in analysis:
        print(f"❌ Error: {analysis['error']}", file=sys.stderr)
        return

    print(f"\n{'='*70}")
    print(f"CI Failure Analysis: {analysis['log_file']}")
    print(f"{'='*70}")
    print(f"Log size: {analysis['total_lines']} lines")
    print(f"Patterns detected: {analysis['patterns_found']}")
    print()

    if analysis['patterns_found'] == 0:
        print("✓ No known failure patterns detected")
        print("  This might be:")
        print("  - A novel failure not yet cataloged")
        print("  - A timeout without specific error")
        print("  - A test that failed silently")
        print("\n  Manual analysis recommended:")
        print("  1. Search for 'ERROR', 'FAIL', 'panic', 'fault' in logs")
        print("  2. Check if expected success signals appear")
        print("  3. Look for the last successful operation before hang/crash")
        return

    print(f"{'─'*70}")

    for i, (name, (pattern, line, line_num)) in enumerate(analysis['matches'].items(), 1):
        print(f"\n[{i}] {name}")
        print(f"    Line {line_num}: {line.strip()}")
        print(f"\n    📊 Diagnosis:")
        for diag_line in pattern.diagnosis.split('\n'):
            print(f"       {diag_line}")
        print(f"\n    🔧 Fix:")
        for fix_line in pattern.fix.split('\n'):
            print(f"       {fix_line}")

        if show_context and analysis['log_content']:
            print(f"\n    📄 Context:")
            context = extract_context(analysis['log_content'], line_num)
            for ctx_line in context.split('\n'):
                print(f"       {ctx_line}")

        print(f"\n    {'─'*66}")

    print(f"\n{'='*70}")


def main():
    parser = argparse.ArgumentParser(
        description='Analyze Breenix CI failure logs',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Analyze CI artifact log
  %(prog)s target/xtask_ring3_smoke_output.txt

  # Show context around failures
  %(prog)s --context target/xtask_ring3_smoke_output.txt

  # Analyze multiple logs
  %(prog)s target/*.txt

  # Find logs directory
  %(prog)s logs/breenix_*.log
        """
    )
    parser.add_argument(
        'log_files',
        nargs='+',
        type=Path,
        help='Log files to analyze'
    )
    parser.add_argument(
        '--context',
        action='store_true',
        help='Show context lines around failures'
    )
    parser.add_argument(
        '--verbose',
        action='store_true',
        help='Verbose output'
    )

    args = parser.parse_args()

    # Analyze each log file
    for log_file in args.log_files:
        if not log_file.exists():
            print(f"❌ File not found: {log_file}", file=sys.stderr)
            continue

        analysis = analyze_log_file(log_file, verbose=args.verbose)
        print_analysis(analysis, show_context=args.context)
        print()


if __name__ == '__main__':
    main()
