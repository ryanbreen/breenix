#!/usr/bin/env python3
"""
Trace memory dump parser and validator for Breenix.

This parses an aarch64 QMP ELF core (with --kernel for symbols) or a raw dump
of the kernel's `TRACE_BUFFERS` array, and validates that the tracing subsystem
actually recorded usable events. Raw dumps are produced by
`scripts/test_tracing_via_gdb.sh`, which owns QEMU and GDB.

The tracing structures it decodes:
- TraceEvent:     16 bytes (u64 timestamp, u16 event_type, u8 cpu_id, u8 flags,
                  u32 payload), #[repr(C, align(16))]
- TraceCpuBuffer: TRACE_BUFFER_SIZE entries + write_idx/read_idx/dropped
                  metadata, #[repr(C, align(64))]
- TraceCounter:   per-CPU u64 slots on 64-byte cache lines

Layout constants and the event-type table are read out of the kernel sources
that define them (`kernel/src/tracing/{core,buffer}.rs`) rather than duplicated
here as literals. A stale copy of either would silently mis-decode: before this
was derived, the table held 13 of the kernel's ~60 event types, so most real
events decoded as UNKNOWN.

Usage:
    python3 scripts/trace_memory_dump.py --parse <dump.bin> [--max-cpus N] --validate
    python3 scripts/trace_memory_dump.py --parse <core.elf> --kernel <kernel-elf> --validate
"""

import re
import sys
import struct
import subprocess
import argparse
from pathlib import Path
from dataclasses import dataclass
from typing import List, Tuple

# ============================================================================
# Constants derived from kernel/src/tracing/
# ============================================================================

# TraceEvent is #[repr(C, align(16))]: 8 + 2 + 1 + 1 + 4 = 16 bytes.
TRACE_EVENT_SIZE = 16
# TraceCpuBuffer metadata following the entries array:
# write_idx(8) + read_idx(8) + dropped(8) + _padding(24).
TRACE_BUFFER_METADATA = 48

BREENIX_ROOT = Path(__file__).resolve().parent.parent
CORE_RS = BREENIX_ROOT / "kernel" / "src" / "tracing" / "core.rs"
BUFFER_RS = BREENIX_ROOT / "kernel" / "src" / "tracing" / "buffer.rs"


def read_usize_const(path: Path, name: str) -> int:
    """Read `pub const <name>: usize = <n>;` out of a kernel source file."""
    pattern = re.compile(r"^pub const %s: usize = ([0-9_]+);" % re.escape(name), re.M)
    match = pattern.search(path.read_text())
    if not match:
        raise SystemExit("Error: could not read %s from %s" % (name, path))
    return int(match.group(1).replace("_", ""))


def read_event_types(core_rs: Path, providers_dir: Path) -> dict:
    """Read every event-type constant the kernel defines.

    Two places define them and both have to be read, or real events decode as
    UNKNOWN:

    * `impl TraceEventType` in core.rs, the shared types (TIMER_TICK, the
      context-switch and syscall families, the debug markers).
    * each provider module, which composes its own ids as
      `((PROVIDER_ID as u16) << 8) | n`. `TEARDOWN_DEFER_EVENT` is 0x0A00 that
      way, and an x86 evidence run failed on exactly that value before this
      function looked at the provider files.

    Returns {value: NAME}. Two providers share an id, so a duplicate value keeps
    the first name seen; that is enough for human-readable decoding.
    """
    table = {}

    text = core_rs.read_text()
    body = re.search(r"impl TraceEventType \{(.*?)\n\}", text, re.S)
    if not body:
        raise SystemExit("Error: could not locate `impl TraceEventType` in %s" % core_rs)
    for name, value in re.findall(
        r"pub const (\w+): u16 = (0[xX][0-9a-fA-F]+|\d+);", body.group(1)
    ):
        table.setdefault(int(value, 0), name)
    if not table:
        raise SystemExit("Error: no event-type constants found in %s" % core_rs)

    for provider in sorted(providers_dir.glob("*.rs")):
        source = provider.read_text()
        provider_id = re.search(
            r"^pub const PROVIDER_ID: u8 = (0[xX][0-9a-fA-F]+|\d+);", source, re.M
        )
        if not provider_id:
            continue
        base = int(provider_id.group(1), 0) << 8
        for name, low in re.findall(
            r"^pub const (\w+): u16 = \(\(PROVIDER_ID as u16\) << 8\) \| "
            r"(0[xX][0-9a-fA-F]+|\d+);",
            source,
            re.M,
        ):
            table.setdefault(base | int(low, 0), name)

    return table


TRACE_BUFFER_SIZE = read_usize_const(BUFFER_RS, "TRACE_BUFFER_SIZE")
KERNEL_MAX_CPUS = read_usize_const(CORE_RS, "MAX_CPUS")
EVENT_TYPES = read_event_types(CORE_RS, CORE_RS.parent / "providers")


@dataclass
class TraceEvent:
    """A single trace event from the ring buffer."""

    timestamp: int
    event_type: int
    cpu_id: int
    flags: int
    payload: int

    @classmethod
    def from_bytes(cls, data: bytes) -> "TraceEvent":
        if len(data) != TRACE_EVENT_SIZE:
            raise ValueError("TraceEvent needs %d bytes, got %d" % (TRACE_EVENT_SIZE, len(data)))
        timestamp, event_type, cpu_id, flags, payload = struct.unpack("<QHBBI", data)
        return cls(
            timestamp=timestamp,
            event_type=event_type,
            cpu_id=cpu_id,
            flags=flags,
            payload=payload,
        )

    def event_name(self) -> str:
        return EVENT_TYPES.get(self.event_type, "UNKNOWN(%#06x)" % self.event_type)

    def is_recorded(self) -> bool:
        """True for a slot a CPU actually wrote, false for an untouched slot."""
        return self.timestamp != 0 or self.event_type != 0

    def __str__(self) -> str:
        return "[%d] cpu%d %s payload=%#x flags=%#x" % (
            self.timestamp,
            self.cpu_id,
            self.event_name(),
            self.payload,
            self.flags,
        )


@dataclass
class TraceCpuBuffer:
    """A per-CPU trace ring buffer."""

    cpu_id: int
    write_idx: int
    dropped: int
    events: List[TraceEvent]

    def count(self) -> int:
        """Number of valid events in the buffer."""
        return min(self.write_idx, TRACE_BUFFER_SIZE)

    def is_empty(self) -> bool:
        return self.write_idx == 0

    def wrapped(self) -> bool:
        return self.write_idx > TRACE_BUFFER_SIZE

    def iter_events(self):
        """Iterate over recorded events, oldest to newest."""
        count = self.count()
        if self.wrapped():
            start = self.write_idx % TRACE_BUFFER_SIZE
            for i in range(count):
                yield self.events[(start + i) % TRACE_BUFFER_SIZE]
        else:
            for i in range(count):
                yield self.events[i]


@dataclass
class TraceCounter:
    """A per-CPU atomic counter."""

    name: str
    per_cpu: List[int]

    def total(self) -> int:
        return sum(self.per_cpu)


def buffer_stride() -> int:
    """Size of one TraceCpuBuffer, including the align(64) tail padding."""
    entries = TRACE_BUFFER_SIZE * TRACE_EVENT_SIZE
    return ((entries + TRACE_BUFFER_METADATA + 63) // 64) * 64


def extract_core_buffers(handle, kernel: str, max_cpus: int) -> bytes:
    """Extract the .bss ring through the aarch64 flat HHDM mapping.

    linker.ld rebases the image by KERNEL_VIRT_BASE; boot.S zeroes .bss
    before enabling the MMU by subtracting that same base. No page-table
    walk is involved. ELF64 sizes/fields below are the ELF file ABI.
    """
    linker = BREENIX_ROOT / "kernel/src/arch_impl/aarch64/linker.ld"
    match = re.search(r"^KERNEL_VIRT_BASE\s*=\s*(0[xX][0-9a-fA-F_]+);",
                      linker.read_text(), re.M)
    if not match:
        raise SystemExit("Error: could not read KERNEL_VIRT_BASE from %s" % linker)
    base = int(match.group(1).replace("_", ""), 16)
    try:
        symbols = subprocess.check_output(["nm", kernel], text=True)
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit("Error: nm could not read %s: %s" % (kernel, error))
    addresses = [int(fields[0], 16) for line in symbols.splitlines()
                 if len(fields := line.split()) == 3 and fields[-1] == "TRACE_BUFFERS"]
    if len(addresses) != 1:
        raise SystemExit("Error: expected exactly one TRACE_BUFFERS symbol in %s" % kernel)
    va = addresses[0]
    pa = va & ~base
    if va < base or pa != va - base:
        raise SystemExit("Error: TRACE_BUFFERS VA %#x is outside the flat HHDM" % va)
    total_size = buffer_stride() * max_cpus
    handle.seek(0)
    header = handle.read(64)
    if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01":
        raise SystemExit("Error: expected a little-endian ELF64 core header")
    fields = struct.unpack("<16sHHIQQQIHHHHHH", header)
    if fields[1] != 4 or fields[2] != 183 or fields[3] != 1:
        raise SystemExit("Error: expected an aarch64 ELF core (ET_CORE)")
    phoff, phentsize, phnum = fields[5], fields[9], fields[10]
    # This host's QEMU core reports e_ehsize=8 despite its full ELF64 header.
    # Read the fixed ABI header above and use e_phoff, not e_ehsize, to
    # locate the table; segment bounds below still require real file bytes.
    if phentsize != 56 or phnum == 0 or phnum == 0xffff:
        raise SystemExit("Error: unsupported ELF program-header layout")
    handle.seek(0, 2)
    file_size = handle.tell()
    if phoff + phentsize * phnum > file_size:
        raise SystemExit("Error: truncated ELF program-header table")
    segments = []
    for index in range(phnum):
        handle.seek(phoff + index * phentsize)
        kind, flags, offset, vaddr, paddr, filesz, memsz, align = struct.unpack(
            "<IIQQQQQQ", handle.read(phentsize))
        if kind == 1:  # PT_LOAD: p_paddr is the guest-physical base.
            segments.append((paddr, filesz, offset))
    for paddr, filesz, offset in segments:
        if paddr <= pa and pa + total_size <= paddr + filesz:
            position = offset + pa - paddr
            if position + total_size > file_size:
                raise SystemExit("Error: truncated PT_LOAD for TRACE_BUFFERS; segments=%r" % segments)
            handle.seek(position)
            data = handle.read(total_size)
            if len(data) != total_size:
                raise SystemExit("Error: short TRACE_BUFFERS read; segments=%r" % segments)
            print("ELF core: TRACE_BUFFERS VA=%#x PA=%#x bytes=%d" % (va, pa, total_size))
            return data
    raise SystemExit("Error: no PT_LOAD contains TRACE_BUFFERS [%#x, %#x); "
                     "segments (p_paddr, p_filesz, p_offset)=%r"
                     % (pa, pa + total_size, segments))


def parse_trace_buffers(data: bytes, max_cpus: int) -> List[TraceCpuBuffer]:
    """Parse a raw memory dump of the TRACE_BUFFERS array."""
    stride = buffer_stride()
    entries_size = TRACE_BUFFER_SIZE * TRACE_EVENT_SIZE
    expected = stride * max_cpus
    if len(data) < expected:
        raise SystemExit(
            "Error: dump is %d bytes but %d CPUs x %d bytes = %d are required. "
            "The dump does not cover the whole TRACE_BUFFERS array."
            % (len(data), max_cpus, stride, expected)
        )

    buffers = []
    for cpu in range(max_cpus):
        block = data[cpu * stride : (cpu + 1) * stride]
        events = [
            TraceEvent.from_bytes(block[i * TRACE_EVENT_SIZE : (i + 1) * TRACE_EVENT_SIZE])
            for i in range(TRACE_BUFFER_SIZE)
        ]
        write_idx = struct.unpack("<Q", block[entries_size : entries_size + 8])[0]
        dropped = struct.unpack("<Q", block[entries_size + 16 : entries_size + 24])[0]
        buffers.append(
            TraceCpuBuffer(cpu_id=cpu, write_idx=write_idx, dropped=dropped, events=events)
        )
    return buffers


def parse_counter(data: bytes, name: str, max_cpus: int) -> TraceCounter:
    """Parse a TraceCounter's per-CPU slots (64-byte cache-line aligned)."""
    per_cpu = []
    for cpu in range(max_cpus):
        offset = cpu * 64
        if offset + 8 > len(data):
            raise SystemExit(
                "Error: counter dump for %s is truncated at CPU %d" % (name, cpu)
            )
        per_cpu.append(struct.unpack("<Q", data[offset : offset + 8])[0])
    return TraceCounter(name=name, per_cpu=per_cpu)


def validate_trace_buffers(buffers: List[TraceCpuBuffer], total_events=None) -> Tuple[bool, List[str]]:
    """Validate that the dump shows a live, correctly-decoded tracing subsystem.

    Every check below can fail. The point of this harness is that an empty or
    garbage buffer is reported as a FAILURE — a run that records nothing must
    not read as evidence that tracing works.
    """
    messages = []
    success = True

    if total_events is None:
        total_events = sum(b.count() for b in buffers)
    if total_events == 0:
        messages.append("FAIL: no trace events recorded in any CPU buffer")
        success = False
    else:
        messages.append(
            "OK: %d total events across %d parsed CPU buffers" % (total_events, len(buffers))
        )

    # The boot CPU always runs; an empty CPU0 buffer means recording never
    # happened, whatever the other buffers hold.
    if buffers and buffers[0].is_empty():
        messages.append("FAIL: CPU0 (boot CPU) recorded no events")
        success = False
    elif buffers:
        messages.append(
            "OK: CPU0 recorded %d events (write_idx=%d, dropped=%d, wrapped=%s)"
            % (
                buffers[0].count(),
                buffers[0].write_idx,
                buffers[0].dropped,
                buffers[0].wrapped(),
            )
        )

    # Decode coverage: every recorded event must map to a known event type.
    # An unknown type means the dump is misaligned or the decode table drifted
    # from the kernel, and either way the decoded output is not trustworthy.
    unknown = {}
    seen_types = set()
    for buffer in buffers:
        for event in buffer.iter_events():
            if not event.is_recorded():
                continue
            seen_types.add(event.event_type)
            if event.event_type not in EVENT_TYPES:
                unknown[event.event_type] = unknown.get(event.event_type, 0) + 1
    if unknown:
        detail = ", ".join(
            "%#06x x%d" % (k, v) for k, v in sorted(unknown.items(), key=lambda kv: -kv[1])[:8]
        )
        messages.append("FAIL: %d event type(s) did not decode: %s" % (len(unknown), detail))
        success = False
    elif seen_types:
        names = sorted(EVENT_TYPES[t] for t in seen_types)
        messages.append("OK: all %d observed event types decode: %s" % (len(names), ", ".join(names)))

    # A booted kernel with the timer running must have recorded timer ticks.
    timer_tick = next((v for v, n in EVENT_TYPES.items() if n == "TIMER_TICK"), None)
    if timer_tick is None:
        messages.append("FAIL: kernel event-type table has no TIMER_TICK")
        success = False
    elif timer_tick not in seen_types:
        messages.append("FAIL: no TIMER_TICK events recorded — the timer provider never fired")
        success = False
    else:
        messages.append("OK: TIMER_TICK events present")

    # Ring-overflow accounting. `TraceCpuBuffer::record` bumps `dropped` every
    # time it reserves a slot that already held an event, so once a buffer has
    # wrapped exactly TRACE_BUFFER_SIZE of its writes are still resident and the
    # rest are counted as dropped. That is an exact identity, and it is the one
    # falsifiable statement this dump can make about the ring's own bookkeeping.
    for buffer in buffers:
        if buffer.wrapped():
            expected_dropped = buffer.write_idx - TRACE_BUFFER_SIZE
            if buffer.dropped != expected_dropped:
                messages.append(
                    "FAIL: CPU%d dropped=%d but write_idx=%d implies %d overwritten slots"
                    % (buffer.cpu_id, buffer.dropped, buffer.write_idx, expected_dropped)
                )
                success = False
            else:
                messages.append(
                    "OK: CPU%d overflow accounting exact (write_idx=%d, dropped=%d, resident=%d)"
                    % (buffer.cpu_id, buffer.write_idx, buffer.dropped, TRACE_BUFFER_SIZE)
                )
        elif buffer.dropped != 0:
            messages.append(
                "FAIL: CPU%d never wrapped (write_idx=%d) but reports dropped=%d"
                % (buffer.cpu_id, buffer.write_idx, buffer.dropped)
            )
            success = False

    # Slot-order timestamp inversions are REPORTED, not gated.
    #
    # `record_event` samples the timestamp before `record()` reserves a slot, and
    # `record()` cannot mask interrupts (it runs in the timer and IRQ paths under
    # a sub-microsecond budget). So an interrupt landing between a writer's
    # timestamp read and its slot write lets the nested event take the earlier
    # slot with the later timestamp. Sampling the timestamp after the reservation
    # would not close it either — the same interrupt can land between the
    # reservation and the timestamp read. Slot order therefore does not imply
    # timestamp order on this ring, and asserting that it does would be a false
    # invariant. The count is printed because a sudden jump in it is still worth
    # seeing, and because a misaligned decode shows up here as mass inversion.
    total_inversions = 0
    for buffer in buffers:
        if buffer.is_empty():
            continue
        last_ts = 0
        inversions = 0
        for event in buffer.iter_events():
            if not event.is_recorded():
                continue
            if event.timestamp < last_ts:
                inversions += 1
            last_ts = event.timestamp
        total_inversions += inversions
        if inversions:
            messages.append(
                "CENSUS: CPU%d has %d nested-record timestamp inversion(s) of %d events"
                % (buffer.cpu_id, inversions, buffer.count())
            )
    messages.append(
        "CENSUS: %d timestamp inversion(s) total (not a failure; see comment)" % total_inversions
    )

    return success, messages


def main():
    parser = argparse.ArgumentParser(
        description="Parse and validate a Breenix TRACE_BUFFERS memory dump"
    )
    parser.add_argument("--parse", "-p", required=True, help="Raw TRACE_BUFFERS dump or aarch64 ELF core to parse")
    parser.add_argument("--kernel", help="Kernel ELF for symbols (required for an ELF core)")
    parser.add_argument("--validate", "-v", action="store_true", help="Validate trace contents")
    parser.add_argument(
        "--max-cpus",
        type=int,
        default=KERNEL_MAX_CPUS,
        help="CPU buffers in the dump (default: MAX_CPUS from kernel/src/tracing/core.rs = %d)"
        % KERNEL_MAX_CPUS,
    )
    parser.add_argument(
        "--events", action="store_true", help="Print every decoded event, not just the summary"
    )
    args = parser.parse_args()

    if args.max_cpus < 1:
        parser.error("--max-cpus must be positive")
    with open(args.parse, "rb") as handle:
        is_core = handle.read(4) == b"\x7fELF"
        handle.seek(0)
        if is_core:
            if not args.kernel:
                parser.error("--kernel is required when --parse names an ELF core")
            data = extract_core_buffers(handle, args.kernel, args.max_cpus)
        else:
            data = handle.read()

    buffers = parse_trace_buffers(data, args.max_cpus)

    print(
        "Layout: TRACE_BUFFER_SIZE=%d, MAX_CPUS=%d, stride=%d bytes, %d event types known"
        % (TRACE_BUFFER_SIZE, KERNEL_MAX_CPUS, buffer_stride(), len(EVENT_TYPES))
    )
    print("\nParsed %d CPU buffers:" % len(buffers))
    for buffer in buffers:
        print(
            "  CPU%d: %d events (write_idx=%d, dropped=%d)"
            % (buffer.cpu_id, buffer.count(), buffer.write_idx, buffer.dropped)
        )

    total_events = sum(b.count() for b in buffers)
    print("TRACE_DECODED_EVENTS:%d" % total_events)

    if args.events:
        print("\nEvents:")
        for buffer in buffers:
            if buffer.is_empty():
                continue
            print("\n--- CPU%d (%d events) ---" % (buffer.cpu_id, buffer.count()))
            for event in buffer.iter_events():
                if event.is_recorded():
                    print("  %s" % event)

    if args.validate:
        success, messages = validate_trace_buffers(buffers, total_events)
        print("\nValidation results:")
        for message in messages:
            print("  %s" % message)
        print("\nTRACE_VALIDATION:%s" % ("PASS" if success else "FAIL"))
        sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
