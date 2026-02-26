#!/usr/bin/env python3
"""Compare Breenix xHCI trace with Linux ftrace for head-to-head analysis.

Usage:
    python3 scripts/compare-xhci-traces.py \\
        --breenix /tmp/breenix-xhci-trace.txt \\
        --linux docs/linux-xhci-trace-raw.txt

Parses both traces and produces a side-by-side comparison of:
  - Command sequence (Enable Slot, Address Device, Configure Endpoint, etc.)
  - Input Context fields (Control, Slot, EP DWORDs)
  - Completion codes
  - Endpoint states
"""

import sys
import re
import argparse
from dataclasses import dataclass, field
from typing import Optional

# Import parser functions from sibling module
sys.path.insert(0, sys.path[0])
from importlib.util import spec_from_file_location, module_from_spec
import os

# Re-use parse logic from parse-xhci-trace.py
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))


def load_parser():
    """Load parse-xhci-trace.py as a module."""
    spec = spec_from_file_location(
        "parse_xhci_trace",
        os.path.join(SCRIPT_DIR, "parse-xhci-trace.py"),
    )
    mod = module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# xHCI TRB type codes
TRB_ENABLE_SLOT = 9
TRB_ADDRESS_DEVICE = 11
TRB_CONFIGURE_ENDPOINT = 12
TRB_STOP_ENDPOINT = 15
TRB_SET_TR_DEQUEUE = 16
TRB_COMMAND_COMPLETION = 33


@dataclass
class LinuxCommand:
    """A command extracted from Linux ftrace."""
    timestamp: float
    trb_type: str  # "Enable Slot", "Address Device", etc.
    slot: int = 0
    flags: str = ""
    completion_code: str = ""
    raw_line: str = ""


@dataclass
class BreenixCommand:
    """A command extracted from Breenix trace."""
    seq: int
    trb_type: int
    trb_type_name: str
    slot: int
    op: str = ""  # CMD_SUBMIT, CMD_COMPLETE, XFER_SUBMIT, XFER_EVENT
    param: int = 0
    cc: int = 0
    cc_name: str = ""


def parse_linux_ftrace(filepath: str) -> list:
    """Parse Linux ftrace to extract xHCI commands and their completions."""
    commands = []

    with open(filepath) as f:
        lines = f.readlines()

    # Patterns for Linux ftrace xHCI events
    cmd_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_queue_trb:\s+CMD:\s+(.+)"
    )
    event_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_handle_event:\s+EVENT:\s+TRB\s+([0-9a-fA-F]+)\s+status\s+'([^']+)'\s+len\s+(\d+)\s+slot\s+(\d+)\s+ep\s+(\d+)\s+type\s+'([^']+)'"
    )
    handle_cmd_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_handle_command:\s+CMD:\s+(.+)"
    )
    ctx_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_(?:address|configure)_ctx:\s+(.+)"
    )
    address_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_address_ctx:\s+(.*?)Ctx Entries\s+(\d+).*?Port#\s+(\d+)/(\d+).*?Addr\s+(\d+)\s+State\s+(\S+)"
    )
    configure_pattern = re.compile(
        r"(\d+\.\d+):\s+xhci_configure_endpoint_ctx:\s+(.+)"
    )

    pending_cmd = None

    for line in lines:
        line = line.strip()

        # Queue TRB (command submission)
        m = cmd_pattern.search(line)
        if m:
            ts = float(m.group(1))
            cmd_str = m.group(2).strip()
            cmd = LinuxCommand(
                timestamp=ts,
                trb_type=cmd_str.split(":")[0].strip() if ":" in cmd_str else cmd_str.split(" flags")[0].strip(),
                raw_line=line,
            )
            # Extract slot from flags like "b:C s:1"
            slot_m = re.search(r's:(\d+)', cmd_str)
            if slot_m:
                cmd.slot = int(slot_m.group(1))
            commands.append(cmd)
            pending_cmd = cmd
            continue

        # Command completion event
        m = event_pattern.search(line)
        if m:
            ts = float(m.group(1))
            status = m.group(3)
            slot = int(m.group(5))
            event_type = m.group(7)
            if event_type == "Command Completion Event" and pending_cmd:
                pending_cmd.completion_code = status
                pending_cmd.slot = slot if slot else pending_cmd.slot
            continue

    return commands


def parse_breenix_trace(filepath: str) -> list:
    """Parse Breenix trace dump to extract commands."""
    parser = load_parser()

    with open(filepath) as f:
        text = f.read()

    trace_section = parser.extract_trace_section(text)
    if not trace_section:
        print(f"ERROR: No XHCI_TRACE_START/END in {filepath}")
        return []

    records = parser.parse_trace(trace_section)
    commands = []

    for rec in records:
        if rec.op in ("CMD_SUBMIT", "CMD_COMPLETE", "XFER_SUBMIT", "XFER_EVENT"):
            if len(rec.data) >= 16:
                trb = parser.parse_trb(rec.data)
                cmd = BreenixCommand(
                    seq=rec.seq,
                    trb_type=trb["trb_type"],
                    trb_type_name=trb["trb_type_name"],
                    slot=trb["slot_id"] or rec.slot,
                    op=rec.op,
                    param=trb["param"],
                    cc=trb["cc"],
                    cc_name=trb["cc_name"],
                )
                commands.append(cmd)

    return commands, records


def compare_command_sequences(breenix_cmds, linux_cmds):
    """Compare the command sequences between Breenix and Linux."""
    print(f"\n{'='*80}")
    print("COMMAND SEQUENCE COMPARISON")
    print(f"{'='*80}")

    # Filter to just CMD_SUBMIT records from Breenix (command ring submissions only)
    breenix_submits = [c for c in breenix_cmds if c.op == "CMD_SUBMIT"]

    # Build a map of CMD_COMPLETE by seq proximity (next CMD_COMPLETE after each CMD_SUBMIT)
    breenix_completions = [c for c in breenix_cmds if c.op == "CMD_COMPLETE"]
    cc_map = {}  # submit_seq -> completion
    for sub in breenix_submits:
        for comp in breenix_completions:
            if comp.seq > sub.seq:
                cc_map[sub.seq] = comp
                break

    print(f"  Breenix: {len(breenix_submits)} command submissions")
    print(f"  Linux:   {len(linux_cmds)} commands")
    print()

    # Compare command types in order
    bi = 0
    li = 0
    matches = 0
    mismatches = 0

    while bi < len(breenix_submits) and li < len(linux_cmds):
        bc = breenix_submits[bi]
        lc = linux_cmds[li]

        b_name = bc.trb_type_name
        l_name = lc.trb_type

        # Normalize names for comparison
        b_norm = b_name.lower().replace(" ", "_").replace("-", "_")
        l_norm = l_name.lower().replace(" ", "_").replace("-", "_").replace("command", "").strip("_")

        matched = False
        # Check if they're the same type of command
        if b_norm == l_norm or b_name in l_name or l_name.startswith(b_name.split()[0]):
            matched = True

        symbol = "  " if matched else "!!"

        # Get completion code for Breenix command
        comp = cc_map.get(bc.seq)
        b_cc = f"CC={comp.cc_name}" if comp else "CC=?"

        print(f"  {symbol} #{bi:3d} Breenix: {b_name:30s} slot={bc.slot:2d}  {b_cc}")
        print(f"  {symbol} #{li:3d} Linux:   {l_name:30s} slot={lc.slot:2d}  CC={lc.completion_code}")

        if matched:
            matches += 1
        else:
            mismatches += 1
        print()

        bi += 1
        li += 1

    print(f"\nSummary: {matches} matches, {mismatches} mismatches")
    if bi < len(breenix_submits):
        print(f"  Breenix has {len(breenix_submits) - bi} extra commands:")
        for i in range(bi, len(breenix_submits)):
            bc = breenix_submits[i]
            comp = cc_map.get(bc.seq)
            b_cc = f"CC={comp.cc_name}" if comp else ""
            print(f"    #{i:3d} {bc.trb_type_name:30s} slot={bc.slot} {b_cc}")
    if li < len(linux_cmds):
        print(f"  Linux has {len(linux_cmds) - li} extra commands:")
        for i in range(li, len(linux_cmds)):
            lc = linux_cmds[i]
            print(f"    #{i:3d} {lc.trb_type:30s} slot={lc.slot} CC={lc.completion_code}")


def compare_input_contexts(breenix_records, ctx_size=64):
    """Display all Input Context snapshots from Breenix trace for manual comparison."""
    print(f"\n{'='*80}")
    print("INPUT CONTEXT SNAPSHOTS (for manual comparison with Linux)")
    print(f"{'='*80}")

    parser = load_parser()

    for rec in breenix_records:
        if rec.op not in ("INPUT_CTX", "OUTPUT_CTX"):
            continue

        ctx_type = "Input" if rec.op == "INPUT_CTX" else "Output"
        print(f"\n--- {ctx_type} Context [seq={rec.seq} slot={rec.slot}] ---")

        entries = parser.parse_context(rec.data, ctx_size)
        if rec.op == "INPUT_CTX":
            labels = ["Control", "Slot"] + [f"EP{i}" for i in range(1, 32)]
        else:
            labels = ["Slot"] + [f"EP{i}" for i in range(1, 32)]

        for i, entry in enumerate(entries):
            if all(dw == 0 for dw in entry) and i > 1:
                continue
            label = labels[i] if i < len(labels) else f"Entry{i}"
            dw_strs = [f"{dw:08X}" for dw in entry[:8]]
            print(f"  {label:10s}: {' '.join(dw_strs)}")

            # Decode key fields
            if label == "Control" and len(entry) >= 2:
                drop_flags = entry[0]
                add_flags = entry[1]
                add_bits = []
                for bit in range(32):
                    if add_flags & (1 << bit):
                        add_bits.append(f"A{bit}")
                print(f"             Drop={drop_flags:#010x} Add={add_flags:#010x} ({'+'.join(add_bits)})")

            if label == "Slot" and len(entry) >= 4:
                dw0 = entry[0]
                ctx_entries = (dw0 >> 27) & 0x1F
                speed = (dw0 >> 20) & 0xF
                route = dw0 & 0xFFFFF
                dw1 = entry[1]
                port = (dw1 >> 16) & 0xFF
                print(f"             CtxEntries={ctx_entries} Speed={speed} Route={route:#07x} Port={port}")

            if label.startswith("EP") and len(entry) >= 5:
                dw0 = entry[0]
                interval = (dw0 >> 16) & 0xFF
                mult = (dw0 >> 8) & 0x3
                ep_state = dw0 & 0x7

                dw1 = entry[1]
                max_pkt = (dw1 >> 16) & 0xFFFF
                max_burst = (dw1 >> 8) & 0xFF
                ep_type = (dw1 >> 3) & 0x7
                cerr = (dw1 >> 1) & 0x3

                dw2 = entry[2]
                dw3 = entry[3]
                tr_deq = ((dw3 & 0xFFFFFFFF) << 32) | (dw2 & 0xFFFFFFF0)
                dcs = dw2 & 1

                dw4 = entry[4]
                avg_trb = dw4 & 0xFFFF
                max_esit_lo = (dw4 >> 16) & 0xFFFF

                ep_type_names = {0: "NotValid", 1: "IsochOut", 2: "BulkOut", 3: "IntrOut",
                                 4: "Control", 5: "IsochIn", 6: "BulkIn", 7: "IntrIn"}
                state_names = {0: "Disabled", 1: "Running", 2: "Halted", 3: "Stopped", 4: "Error"}

                print(f"             State={state_names.get(ep_state, '?')} Type={ep_type_names.get(ep_type, '?')} "
                      f"MaxPkt={max_pkt} MaxBurst={max_burst} CErr={cerr} Interval={interval} Mult={mult}")
                print(f"             TRDeq={tr_deq:#018x} DCS={dcs} AvgTRB={avg_trb} MaxESIT_Lo={max_esit_lo}")


def main():
    ap = argparse.ArgumentParser(description="Compare Breenix and Linux xHCI traces")
    ap.add_argument("--breenix", required=True, help="Breenix serial log or extracted trace")
    ap.add_argument("--linux", help="Linux ftrace file (optional)")
    ap.add_argument("--ctx-size", type=int, default=32, help="Context entry size (32 or 64)")
    args = ap.parse_args()

    # Parse Breenix trace
    print(f"Parsing Breenix trace: {args.breenix}")
    breenix_cmds, breenix_records = parse_breenix_trace(args.breenix)
    print(f"  Found {len(breenix_cmds)} command/event records, {len(breenix_records)} total records")

    # Parse Linux ftrace if provided
    if args.linux:
        print(f"Parsing Linux ftrace: {args.linux}")
        linux_cmds = parse_linux_ftrace(args.linux)
        print(f"  Found {len(linux_cmds)} commands")

        # Command sequence comparison
        compare_command_sequences(breenix_cmds, linux_cmds)

    # Always show Input/Output Context details
    compare_input_contexts(breenix_records, args.ctx_size)

    # Show completion code summary
    print(f"\n{'='*80}")
    print("COMPLETION CODE SUMMARY")
    print(f"{'='*80}")
    for rec in breenix_records:
        if rec.op in ("CMD_COMPLETE", "XFER_EVENT") and len(rec.data) >= 16:
            parser = load_parser()
            trb = parser.parse_trb(rec.data)
            cc = trb["cc"]
            cc_name = trb["cc_name"]
            trb_type_name = trb["trb_type_name"]
            slot = trb["slot_id"]
            ep = trb["endpoint"]
            if cc != 1 and cc != 13:  # Not SUCCESS or SHORT_PACKET
                print(f"  !! seq={rec.seq:4d} {rec.op:14s} {trb_type_name:30s} slot={slot} ep={ep} CC={cc} ({cc_name})")
            else:
                print(f"     seq={rec.seq:4d} {rec.op:14s} {trb_type_name:30s} slot={slot} ep={ep} CC={cc} ({cc_name})")


if __name__ == "__main__":
    main()
