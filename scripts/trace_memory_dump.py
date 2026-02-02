#!/usr/bin/env python3
"""
Trace Memory Dump Framework for Breenix

This script captures and validates the tracing subsystem state by:
1. Running QEMU with GDB attached
2. Setting a breakpoint at a test completion point
3. Dumping the tracing memory regions (TRACE_BUFFERS, counters, etc.)
4. Parsing the raw bytes to extract and validate trace events

The tracing structures are:
- TraceEvent: 16 bytes (u64 timestamp, u16 event_type, u8 cpu_id, u8 flags, u32 payload)
- TraceCpuBuffer: 1024 entries per CPU + metadata
- TraceCounter: per-CPU atomic counters with 64-byte cache-line alignment

Usage:
    # Dump tracing state at a breakpoint
    python3 scripts/trace_memory_dump.py --breakpoint kernel::kernel_main

    # Dump and validate expected events
    python3 scripts/trace_memory_dump.py --validate
"""

import os
import sys
import struct
import argparse
import subprocess
import time
import json
from pathlib import Path
from dataclasses import dataclass
from typing import List, Optional, Dict, Any, Tuple

# ============================================================================
# Constants matching kernel/src/tracing/
# ============================================================================

# TraceEvent structure (16 bytes, aligned)
# Layout: timestamp(8) + event_type(2) + cpu_id(1) + flags(1) + payload(4)
TRACE_EVENT_SIZE = 16

# Buffer constants
TRACE_BUFFER_SIZE = 1024  # Events per CPU
MAX_CPUS = 8

# Event type constants (from core.rs)
EVENT_TYPES = {
    0x0001: "CTX_SWITCH_ENTRY",
    0x0002: "CTX_SWITCH_EXIT",
    0x0003: "CTX_SWITCH_TO_USER",
    0x0004: "CTX_SWITCH_FROM_USER",
    0x0100: "IRQ_ENTRY",
    0x0101: "IRQ_EXIT",
    0x0102: "TIMER_TICK",
    0x0200: "SCHED_PICK",
    0x0201: "SCHED_RESCHED",
    0x0300: "SYSCALL_ENTRY",
    0x0301: "SYSCALL_EXIT",
    0xFF00: "MARKER",
    0xFF01: "DEBUG",
}

# PIE kernel base address
KERNEL_BASE = 0x10000000000


@dataclass
class TraceEvent:
    """Represents a single trace event from the ring buffer."""
    timestamp: int
    event_type: int
    cpu_id: int
    flags: int
    payload: int

    @classmethod
    def from_bytes(cls, data: bytes) -> 'TraceEvent':
        """Parse a TraceEvent from 16 bytes of memory."""
        if len(data) != TRACE_EVENT_SIZE:
            raise ValueError(f"Expected {TRACE_EVENT_SIZE} bytes, got {len(data)}")

        # Little-endian: timestamp(8), event_type(2), cpu_id(1), flags(1), payload(4)
        timestamp, event_type, cpu_id, flags, payload = struct.unpack('<QHBBI', data)
        return cls(timestamp, event_type, cpu_id, flags, payload)

    def event_name(self) -> str:
        """Get human-readable event type name."""
        return EVENT_TYPES.get(self.event_type, f"UNKNOWN({self.event_type:#06x})")

    def __str__(self) -> str:
        return (f"CPU{self.cpu_id} ts={self.timestamp} "
                f"type={self.event_type:#06x} {self.event_name()} "
                f"payload={self.payload} flags={self.flags:#04x}")


@dataclass
class TraceCpuBuffer:
    """Represents a per-CPU trace ring buffer."""
    cpu_id: int
    write_idx: int
    events: List[TraceEvent]

    def count(self) -> int:
        """Number of valid events in buffer."""
        return min(self.write_idx, TRACE_BUFFER_SIZE)

    def is_empty(self) -> bool:
        return self.write_idx == 0

    def iter_events(self):
        """Iterate over events in order (oldest to newest)."""
        count = self.count()
        if self.write_idx > TRACE_BUFFER_SIZE:
            # Buffer wrapped - start from oldest
            start = self.write_idx % TRACE_BUFFER_SIZE
            for i in range(count):
                idx = (start + i) % TRACE_BUFFER_SIZE
                yield self.events[idx]
        else:
            # Buffer not wrapped - start from 0
            for i in range(count):
                yield self.events[i]


@dataclass
class TraceCounter:
    """Represents a per-CPU atomic counter."""
    name: str
    per_cpu: List[int]  # Value per CPU

    def total(self) -> int:
        return sum(self.per_cpu)


class TraceMemoryDumper:
    """Dumps and parses tracing memory from a running Breenix kernel."""

    def __init__(self, breenix_root: Path):
        self.breenix_root = breenix_root
        self.gdb_script_dir = breenix_root / "breenix-gdb-chat" / "scripts"
        self.symbols: Dict[str, int] = {}

    def get_symbol_address(self, symbol: str) -> Optional[int]:
        """Get the runtime address of a kernel symbol."""
        # Use nm to find symbol offset, then add kernel base
        kernel_binary = self.breenix_root / "target" / "release" / "qemu-uefi"

        try:
            result = subprocess.run(
                ["nm", str(kernel_binary)],
                capture_output=True, text=True, check=True
            )
            for line in result.stdout.splitlines():
                parts = line.split()
                if len(parts) >= 3 and parts[2] == symbol:
                    offset = int(parts[0], 16)
                    return KERNEL_BASE + offset
        except subprocess.CalledProcessError:
            pass
        return None

    def dump_memory_via_gdb(self, address: int, size: int, output_file: Path) -> bool:
        """Use GDB to dump a memory region to a file."""
        gdb_commands = f"""
set pagination off
target remote localhost:1234
dump binary memory {output_file} {address:#x} {address + size:#x}
quit
"""
        try:
            result = subprocess.run(
                ["gdb", "-batch", "-ex", gdb_commands.replace('\n', ' -ex ')],
                capture_output=True, text=True, timeout=30
            )
            return output_file.exists()
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            return False

    def parse_trace_buffers(self, data: bytes) -> List[TraceCpuBuffer]:
        """Parse raw memory dump of TRACE_BUFFERS array."""
        buffers = []

        # TraceCpuBuffer layout (simplified):
        # - entries: [TraceEvent; 1024] = 16384 bytes
        # - write_idx: AtomicUsize = 8 bytes
        # - read_idx: AtomicUsize = 8 bytes (unused)
        # - dropped: AtomicU64 = 8 bytes
        # - padding to 64-byte alignment

        # Calculate buffer size (must be 64-byte aligned)
        entry_data_size = TRACE_BUFFER_SIZE * TRACE_EVENT_SIZE  # 16384
        metadata_size = 8 + 8 + 8 + 24  # write_idx + read_idx + dropped + padding
        buffer_size = entry_data_size + metadata_size  # ~16432, rounded up
        # Actual size with 64-byte alignment
        buffer_size = ((buffer_size + 63) // 64) * 64

        for cpu in range(MAX_CPUS):
            offset = cpu * buffer_size
            if offset + buffer_size > len(data):
                break

            buffer_data = data[offset:offset + buffer_size]

            # Parse events
            events = []
            for i in range(TRACE_BUFFER_SIZE):
                event_offset = i * TRACE_EVENT_SIZE
                event_data = buffer_data[event_offset:event_offset + TRACE_EVENT_SIZE]
                if len(event_data) == TRACE_EVENT_SIZE:
                    events.append(TraceEvent.from_bytes(event_data))

            # Parse write index (after events)
            write_idx_offset = entry_data_size
            write_idx = struct.unpack('<Q', buffer_data[write_idx_offset:write_idx_offset + 8])[0]

            buffers.append(TraceCpuBuffer(cpu_id=cpu, write_idx=write_idx, events=events))

        return buffers

    def parse_counter(self, data: bytes, name: str) -> TraceCounter:
        """Parse a TraceCounter from raw memory."""
        # CpuCounterSlot is 64 bytes (8-byte value + 56 bytes padding)
        per_cpu = []
        for cpu in range(MAX_CPUS):
            offset = cpu * 64  # 64-byte cache-line aligned slots
            if offset + 8 <= len(data):
                value = struct.unpack('<Q', data[offset:offset + 8])[0]
                per_cpu.append(value)
            else:
                per_cpu.append(0)
        return TraceCounter(name=name, per_cpu=per_cpu)


def run_gdb_dump(breenix_root: Path, breakpoint: str = None) -> Dict[str, Any]:
    """
    Run QEMU with GDB, optionally set breakpoint, and dump tracing state.

    Returns a dictionary with:
    - 'buffers': List of TraceCpuBuffer data
    - 'counters': Dict of counter name -> TraceCounter
    - 'enabled': Whether tracing is enabled
    """
    dumper = TraceMemoryDumper(breenix_root)

    # Find symbol addresses
    trace_buffers_addr = dumper.get_symbol_address("TRACE_BUFFERS")
    trace_enabled_addr = dumper.get_symbol_address("TRACE_ENABLED")

    if not trace_buffers_addr:
        print("Error: Could not find TRACE_BUFFERS symbol", file=sys.stderr)
        return {}

    print(f"TRACE_BUFFERS at {trace_buffers_addr:#x}")
    if trace_enabled_addr:
        print(f"TRACE_ENABLED at {trace_enabled_addr:#x}")

    # Calculate buffer size to dump
    entry_data_size = TRACE_BUFFER_SIZE * TRACE_EVENT_SIZE
    metadata_size = 64  # Aligned metadata
    buffer_size = ((entry_data_size + metadata_size + 63) // 64) * 64
    total_size = buffer_size * MAX_CPUS

    print(f"Buffer size per CPU: {buffer_size} bytes")
    print(f"Total dump size: {total_size} bytes")

    # TODO: Implement actual GDB session with breakpoint
    # For now, return placeholder
    return {
        'trace_buffers_addr': trace_buffers_addr,
        'trace_enabled_addr': trace_enabled_addr,
        'buffer_size': buffer_size,
        'total_size': total_size,
    }


def validate_trace_buffers(buffers: List[TraceCpuBuffer]) -> Tuple[bool, List[str]]:
    """
    Validate trace buffer contents against expected patterns.

    Returns (success, list of validation messages).
    """
    messages = []
    success = True

    # Check that at least one buffer has events
    total_events = sum(b.count() for b in buffers)
    if total_events == 0:
        messages.append("FAIL: No trace events recorded")
        success = False
    else:
        messages.append(f"OK: {total_events} total events across {len(buffers)} CPUs")

    # Check for expected event types
    event_types_seen = set()
    for buffer in buffers:
        for event in buffer.iter_events():
            event_types_seen.add(event.event_type)

    # We expect at least timer ticks if the kernel booted
    if 0x0102 not in event_types_seen:  # TIMER_TICK
        messages.append("WARN: No TIMER_TICK events seen")
    else:
        messages.append("OK: TIMER_TICK events present")

    # Check buffer integrity
    for buffer in buffers:
        if buffer.write_idx > 0:
            # Verify timestamps are monotonically increasing (within reason)
            last_ts = 0
            out_of_order = 0
            for event in buffer.iter_events():
                if event.timestamp < last_ts and event.timestamp != 0:
                    out_of_order += 1
                last_ts = event.timestamp

            if out_of_order > 0:
                messages.append(f"WARN: CPU{buffer.cpu_id} has {out_of_order} out-of-order timestamps")
            else:
                messages.append(f"OK: CPU{buffer.cpu_id} timestamps monotonic")

    return success, messages


def main():
    parser = argparse.ArgumentParser(description="Dump and validate Breenix tracing state")
    parser.add_argument("--breakpoint", "-b", help="GDB breakpoint to set before dumping")
    parser.add_argument("--validate", "-v", action="store_true", help="Validate trace contents")
    parser.add_argument("--output", "-o", help="Output file for raw memory dump")
    parser.add_argument("--parse", "-p", help="Parse existing memory dump file")
    args = parser.parse_args()

    # Find breenix root
    script_path = Path(__file__).resolve()
    breenix_root = script_path.parent.parent

    if args.parse:
        # Parse existing dump file
        dumper = TraceMemoryDumper(breenix_root)
        with open(args.parse, 'rb') as f:
            data = f.read()

        buffers = dumper.parse_trace_buffers(data)

        print(f"\nParsed {len(buffers)} CPU buffers:")
        for buffer in buffers:
            print(f"  CPU{buffer.cpu_id}: {buffer.count()} events (write_idx={buffer.write_idx})")

        if args.validate:
            success, messages = validate_trace_buffers(buffers)
            print("\nValidation results:")
            for msg in messages:
                print(f"  {msg}")
            sys.exit(0 if success else 1)

        # Print events
        print("\nEvents:")
        for buffer in buffers:
            if not buffer.is_empty():
                print(f"\n--- CPU{buffer.cpu_id} ({buffer.count()} events) ---")
                for event in buffer.iter_events():
                    if event.timestamp > 0 or event.event_type > 0:
                        print(f"  {event}")
    else:
        # Run GDB and dump
        result = run_gdb_dump(breenix_root, args.breakpoint)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
