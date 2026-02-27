#!/usr/bin/env python3
"""Parse Breenix xHCI trace dump from serial log.

Usage:
    python3 scripts/parse-xhci-trace.py /tmp/breenix-parallels-serial.log
    python3 scripts/parse-xhci-trace.py /tmp/breenix-xhci-trace.txt

Extracts the trace section between XHCI_TRACE_START and XHCI_TRACE_END,
parses each record, and prints a human-readable summary.
"""

import sys
import re
from dataclasses import dataclass, field
from typing import Optional


# xHCI TRB type names
TRB_TYPES = {
    1: "Normal",
    2: "Setup Stage",
    3: "Data Stage",
    4: "Status Stage",
    6: "Link",
    8: "No-Op",
    9: "Enable Slot",
    10: "Disable Slot",
    11: "Address Device",
    12: "Configure Endpoint",
    13: "Evaluate Context",
    14: "Reset Endpoint",
    15: "Stop Endpoint",
    16: "Set TR Dequeue Pointer",
    23: "No-Op",
    32: "Transfer Event",
    33: "Command Completion",
    34: "Port Status Change",
}

CC_NAMES = {
    1: "SUCCESS",
    4: "USB_TRANSACTION_ERROR",
    6: "STALL_ERROR",
    12: "ENDPOINT_NOT_ENABLED",
    13: "SHORT_PACKET",
}


@dataclass
class TraceRecord:
    seq: int
    op: str
    slot: int
    dci: int
    timestamp: int
    data_len: int
    data: bytes = field(default_factory=bytes)
    note: str = ""


def parse_trb(data: bytes) -> dict:
    """Parse a 16-byte TRB into param/status/control fields."""
    if len(data) < 16:
        return {}
    param = int.from_bytes(data[0:8], "little")
    status = int.from_bytes(data[8:12], "little")
    control = int.from_bytes(data[12:16], "little")
    trb_type = (control >> 10) & 0x3F
    cc = (status >> 24) & 0xFF
    slot_id = (control >> 24) & 0xFF
    ep = (control >> 16) & 0x1F
    return {
        "param": param,
        "status": status,
        "control": control,
        "trb_type": trb_type,
        "trb_type_name": TRB_TYPES.get(trb_type, f"Unknown({trb_type})"),
        "cc": cc,
        "cc_name": CC_NAMES.get(cc, f"Unknown({cc})"),
        "slot_id": slot_id,
        "endpoint": ep,
    }


def parse_context(data: bytes, ctx_size: int = 64) -> list:
    """Parse context bytes into DWORDs grouped by context entry."""
    entries = []
    offset = 0
    while offset + ctx_size <= len(data):
        entry_dwords = []
        for dw_off in range(0, ctx_size, 4):
            if offset + dw_off + 4 <= len(data):
                dw = int.from_bytes(data[offset + dw_off : offset + dw_off + 4], "little")
                entry_dwords.append(dw)
        entries.append(entry_dwords)
        offset += ctx_size
    return entries


def format_trb(trb: dict, label: str = "") -> str:
    """Format a parsed TRB for display."""
    parts = [f"{label}" if label else ""]
    parts.append(f"type={trb['trb_type_name']}")
    parts.append(f"param={trb['param']:#018x}")
    if trb["cc"]:
        parts.append(f"CC={trb['cc_name']}")
    if trb["slot_id"]:
        parts.append(f"slot={trb['slot_id']}")
    if trb["endpoint"]:
        parts.append(f"ep={trb['endpoint']}")
    return " ".join(parts)


def format_context_entry(dwords: list, label: str) -> str:
    """Format a context entry (list of DWORDs) for display."""
    dw_strs = [f"{dw:08X}" for dw in dwords[:8]]  # First 8 DWORDs max
    return f"  {label}: {' '.join(dw_strs)}"


def extract_trace_section(text: str) -> str:
    """Extract text between XHCI_TRACE_START and XHCI_TRACE_END markers."""
    start = text.find("=== XHCI_TRACE_START")
    end = text.find("=== XHCI_TRACE_END ===")
    if start < 0 or end < 0:
        return ""
    return text[start:end + len("=== XHCI_TRACE_END ===")]


def parse_trace(trace_text: str) -> list:
    """Parse the trace dump into a list of TraceRecord objects."""
    records = []
    current_record = None
    hex_data = bytearray()

    for line in trace_text.split("\n"):
        line = line.strip()
        if not line or line.startswith("===") or line == "(no records)":
            continue

        # Record header: T NNNN OP_NAME S=NN E=NN TS=XXXXXXXXXXXXXXXX LEN=XXXX
        m = re.match(
            r"T\s+(\d+)\s+(\S+)\s+S=(\d+)\s+E=(\d+)\s+TS=([0-9A-Fa-f]+)\s+LEN=([0-9A-Fa-f]+)",
            line,
        )
        if m:
            # Save previous record's data
            if current_record is not None:
                current_record.data = bytes(hex_data)
                records.append(current_record)
                hex_data = bytearray()

            current_record = TraceRecord(
                seq=int(m.group(1)),
                op=m.group(2),
                slot=int(m.group(3)),
                dci=int(m.group(4)),
                timestamp=int(m.group(5), 16),
                data_len=int(m.group(6), 16),
            )
            continue

        # Note string: "some text"
        if current_record and current_record.op == "NOTE" and line.startswith('"'):
            current_record.note = line.strip('"')
            continue

        # Hex data line: all hex digits and spaces (after strip, no leading whitespace)
        if current_record and re.match(r"^[0-9A-Fa-f][0-9A-Fa-f ]+$", line):
            # Parse hex bytes from groups separated by spaces
            hex_str = line.replace(" ", "")
            try:
                hex_data.extend(bytes.fromhex(hex_str))
            except ValueError:
                pass

    # Don't forget the last record
    if current_record is not None:
        current_record.data = bytes(hex_data)
        records.append(current_record)

    return records


def print_records(records: list, ctx_size: int = 64):
    """Print trace records in a human-readable format."""
    print(f"\n{'='*80}")
    print(f"Breenix xHCI Trace: {len(records)} records")
    print(f"{'='*80}\n")

    for rec in records:
        ts_delta = ""  # Could compute deltas if needed
        header = f"[{rec.seq:4d}] {rec.op:12s} slot={rec.slot} dci={rec.dci} ts={rec.timestamp:#018x}"

        if rec.op == "NOTE":
            print(f"{header}  \"{rec.note}\"")
            continue

        if rec.op in ("CMD_SUBMIT", "CMD_COMPLETE", "XFER_SUBMIT", "XFER_EVENT", "SET_TR_DEQ"):
            if len(rec.data) >= 16:
                trb = parse_trb(rec.data)
                print(f"{header}  {format_trb(trb)}")
            else:
                print(header)
            continue

        if rec.op == "DOORBELL":
            if len(rec.data) >= 8:
                addr = int.from_bytes(rec.data[0:8], "little")
                print(f"{header}  addr={addr:#018x} target={rec.dci}")
            else:
                print(header)
            continue

        if rec.op in ("MMIO_W32",):
            if len(rec.data) >= 12:
                addr = int.from_bytes(rec.data[0:8], "little")
                val = int.from_bytes(rec.data[8:12], "little")
                print(f"{header}  [{addr:#018x}] <- {val:#010x}")
            else:
                print(header)
            continue

        if rec.op in ("MMIO_W64",):
            if len(rec.data) >= 16:
                addr = int.from_bytes(rec.data[0:8], "little")
                val = int.from_bytes(rec.data[8:16], "little")
                print(f"{header}  [{addr:#018x}] <- {val:#018x}")
            else:
                print(header)
            continue

        if rec.op in ("INPUT_CTX", "OUTPUT_CTX"):
            print(header)
            entries = parse_context(rec.data, ctx_size)
            labels = ["Control", "Slot"] + [f"EP{i}" for i in range(1, 32)]
            if rec.op == "OUTPUT_CTX":
                labels = ["Slot"] + [f"EP{i}" for i in range(1, 32)]
            for i, entry in enumerate(entries):
                if i < len(labels):
                    label = labels[i]
                else:
                    label = f"Entry{i}"
                # Skip all-zero entries beyond slot+control
                if all(dw == 0 for dw in entry) and i > 1:
                    continue
                print(format_context_entry(entry, label))
            continue

        if rec.op == "EP_STATE":
            state = rec.data[0] if rec.data else 0
            state_names = {0: "Disabled", 1: "Running", 2: "Halted", 3: "Stopped", 4: "Error"}
            print(f"{header}  state={state} ({state_names.get(state, 'Unknown')})")
            continue

        if rec.op == "CACHE_OP":
            if len(rec.data) >= 12:
                addr = int.from_bytes(rec.data[0:8], "little")
                length = int.from_bytes(rec.data[8:12], "little")
                print(f"{header}  addr={addr:#018x} len={length}")
            else:
                print(header)
            continue

        # Default: just print header
        print(header)


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <serial-log-or-trace-file>")
        sys.exit(1)

    with open(sys.argv[1], "r") as f:
        text = f.read()

    trace_section = extract_trace_section(text)
    if not trace_section:
        print("ERROR: No XHCI_TRACE_START/END markers found in input")
        sys.exit(1)

    records = parse_trace(trace_section)
    if not records:
        print("No trace records found")
        sys.exit(1)

    # Try to detect context size from data
    ctx_size = 32  # Parallels xHCI uses CSZ=0 (32-byte context entries)
    print_records(records, ctx_size)

    # Summary statistics
    op_counts = {}
    for r in records:
        op_counts[r.op] = op_counts.get(r.op, 0) + 1
    print(f"\n{'='*80}")
    print("Operation Summary:")
    for op, count in sorted(op_counts.items()):
        print(f"  {op:20s}: {count}")


if __name__ == "__main__":
    main()
