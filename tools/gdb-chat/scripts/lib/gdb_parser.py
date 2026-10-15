#!/usr/bin/env python3
"""
GDB output parser for structured data extraction.
Converts GDB console output into JSON-friendly dictionaries.
"""

import re
from typing import Dict, List, Any, Optional


def parse_registers(output: str) -> Dict[str, str]:
    """
    Parse 'info registers' output into structured dict.

    Example input:
    rax            0x0    0
    rbx            0xffff800000010000    281474976776192

    Returns: {"rax": "0x0", "rbx": "0xffff800000010000", ...}
    """
    registers = {}

    # Match lines like: "rax            0x0    0"
    # or: "rax            0x0    <some_symbol>"
    pattern = r'^(\w+)\s+(0x[0-9a-fA-F]+)'

    for line in output.split('\n'):
        line = line.strip()
        match = re.match(pattern, line)
        if match:
            reg_name = match.group(1)
            hex_value = match.group(2)
            registers[reg_name] = hex_value

    return registers


def parse_backtrace(output: str) -> List[Dict[str, Any]]:
    """
    Parse backtrace output into list of frames.

    Example input:
    #0  0xffffffff80001234 in kernel_main () at kernel/src/main.rs:42
    #1  0xffffffff80001000 in _start () at kernel/src/boot.asm:10

    Returns: [{"number": 0, "address": "0x...", "function": "kernel_main", ...}, ...]
    """
    frames = []

    # Pattern for frames with source info
    pattern_with_source = r'#(\d+)\s+(0x[0-9a-fA-F]+)\s+in\s+(\S+)\s*\(([^)]*)\)(?:\s+at\s+([^:]+):(\d+))?'

    # Pattern for frames without source (just address)
    pattern_no_source = r'#(\d+)\s+(0x[0-9a-fA-F]+)\s+in\s+(\S+)'

    for line in output.split('\n'):
        line = line.strip()

        # Try pattern with source first
        match = re.search(pattern_with_source, line)
        if match:
            frame = {
                "number": int(match.group(1)),
                "address": match.group(2),
                "function": match.group(3),
                "args": match.group(4) if match.group(4) else "",
            }
            if match.group(5):
                frame["file"] = match.group(5)
                frame["line"] = int(match.group(6))
            frames.append(frame)
            continue

        # Try simpler pattern
        match = re.search(pattern_no_source, line)
        if match:
            frames.append({
                "number": int(match.group(1)),
                "address": match.group(2),
                "function": match.group(3),
            })

    return frames


def parse_memory(output: str) -> List[Dict[str, str]]:
    """
    Parse memory examination (x/ command) output.

    Example input:
    0xffff800000010000:    0x0000000000000000
    0xffff800000010008:    0x0000000000000001

    Returns: [{"address": "0x...", "value": "0x..."}, ...]
    """
    values = []

    # Match lines like: "0xffff800000010000:    0x0000000000000000"
    # or: "0xffff800000010000 <symbol>:    0x0000000000000000"
    pattern = r'(0x[0-9a-fA-F]+)(?:\s+<[^>]+>)?:\s+(.*)'

    for line in output.split('\n'):
        line = line.strip()
        match = re.match(pattern, line)
        if match:
            address = match.group(1)
            rest = match.group(2)

            # Extract all hex values from the rest of the line
            hex_values = re.findall(r'0x[0-9a-fA-F]+', rest)

            for i, val in enumerate(hex_values):
                # Calculate the actual address for each value
                offset = i * 8  # Assuming 8-byte values (x/g)
                actual_addr = hex(int(address, 16) + offset)
                values.append({
                    "address": actual_addr,
                    "value": val
                })

    return values


def parse_breakpoint_set(output: str) -> Optional[Dict[str, Any]]:
    """
    Parse breakpoint creation output.

    Example input:
    Breakpoint 1 at 0xffffffff80001234: file kernel/src/main.rs, line 42.

    Returns: {"number": 1, "address": "0x...", "file": "...", "line": 42}
    """
    pattern = r'Breakpoint\s+(\d+)\s+at\s+(0x[0-9a-fA-F]+)(?::\s+file\s+([^,]+),\s+line\s+(\d+))?'

    match = re.search(pattern, output)
    if match:
        result = {
            "number": int(match.group(1)),
            "address": match.group(2),
        }
        if match.group(3):
            result["file"] = match.group(3)
            result["line"] = int(match.group(4))
        return result

    return None


def parse_stopped(output: str) -> Optional[Dict[str, Any]]:
    """
    Parse output when execution stops (breakpoint hit, signal, etc.).

    Example:
    Breakpoint 1, kernel_main () at kernel/src/main.rs:42
    42          let x = 1;

    Returns: {"reason": "breakpoint", "function": "kernel_main", ...}
    """
    result = {}

    # Check for breakpoint hit
    bp_pattern = r'Breakpoint\s+(\d+),\s+(\S+)\s*\(([^)]*)\)(?:\s+at\s+([^:]+):(\d+))?'
    match = re.search(bp_pattern, output)
    if match:
        result["reason"] = "breakpoint"
        result["breakpoint_number"] = int(match.group(1))
        result["function"] = match.group(2)
        result["args"] = match.group(3)
        if match.group(4):
            result["file"] = match.group(4)
            result["line"] = int(match.group(5))
        return result

    # Check for signal
    signal_pattern = r'Program received signal\s+(\S+),\s+(.+)\.'
    match = re.search(signal_pattern, output)
    if match:
        result["reason"] = "signal"
        result["signal"] = match.group(1)
        result["description"] = match.group(2)
        return result

    # Check for program exit
    exit_pattern = r'Program exited (?:normally|with code\s+(\d+))'
    match = re.search(exit_pattern, output)
    if match:
        result["reason"] = "exited"
        result["exit_code"] = int(match.group(1)) if match.group(1) else 0
        return result

    return None


def truncate_output(text: str, max_lines: int = 100) -> str:
    """
    Truncate large output to prevent token overflow.
    Preserves beginning and end, omits middle.

    Inspired by ChatDBG's proportional truncation approach.
    """
    if not text:
        return text

    lines = text.split('\n')

    if len(lines) <= max_lines:
        return text

    # Keep first 40%, skip middle, keep last 40%
    keep_top = int(max_lines * 0.4)
    keep_bottom = int(max_lines * 0.4)
    omitted = len(lines) - keep_top - keep_bottom

    result = '\n'.join(lines[:keep_top])
    result += f"\n\n... [{omitted} lines omitted] ...\n\n"
    result += '\n'.join(lines[-keep_bottom:])

    return result


def format_registers_table(registers: Dict[str, str]) -> str:
    """
    Format registers dict as a readable table.

    Returns:
    RAX: 0x0000000000000000  RBX: 0xffff800000010000
    RCX: 0x0000000000000001  RDX: 0x0000000000000000
    """
    # Group into pairs for compact display
    items = list(registers.items())
    lines = []

    for i in range(0, len(items), 2):
        line_parts = []
        for j in range(2):
            if i + j < len(items):
                name, value = items[i + j]
                line_parts.append(f"{name.upper():4}: {value:18}")
        lines.append("  ".join(line_parts))

    return '\n'.join(lines)
