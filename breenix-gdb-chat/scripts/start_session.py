#!/usr/bin/env python3
"""
Start a GDB debugging session for Breenix kernel.

Usage:
    start_session.py [--mode uefi|bios] [--timeout SECONDS] [--kernel PATH]

Output (JSON):
    {
        "session_id": "gdb_20251204_143022",
        "gdb_pid": 12345,
        "qemu_pid": 12346,
        "status": "connected",
        "kernel_binary": "/path/to/kernel"
    }
"""

import argparse
import sys
import json
from datetime import datetime
from pathlib import Path

# Add lib directory to path
lib_dir = Path(__file__).parent / "lib"
sys.path.insert(0, str(lib_dir))

from gdb_controller import GDBSession


def find_kernel_binary(mode: str) -> Path:
    """Find the kernel binary, preferring debug build."""
    breenix_dir = Path.home() / "fun/code/breenix"

    # Prefer debug build for symbols
    debug_kernel = breenix_dir / "target/x86_64-breenix/debug/kernel"
    if debug_kernel.exists():
        return debug_kernel

    # Fall back to release
    release_kernel = breenix_dir / "target/x86_64-breenix/release/kernel"
    if release_kernel.exists():
        return release_kernel

    # Try to find via glob
    for kernel in breenix_dir.glob("target/**/kernel"):
        if kernel.is_file():
            return kernel

    raise FileNotFoundError("Kernel binary not found. Run 'cargo build' first.")


def main():
    parser = argparse.ArgumentParser(
        description='Start GDB debugging session for Breenix kernel'
    )
    parser.add_argument(
        '--mode',
        choices=['uefi', 'bios'],
        default='uefi',
        help='Boot mode (default: uefi)'
    )
    parser.add_argument(
        '--timeout',
        type=int,
        default=300,
        help='Session timeout in seconds (default: 300)'
    )
    parser.add_argument(
        '--kernel',
        type=Path,
        default=None,
        help='Path to kernel binary (auto-detected if not specified)'
    )
    parser.add_argument(
        '--debug',
        action='store_true',
        help='Print debug output to stderr'
    )

    args = parser.parse_args()

    # Find kernel binary
    if args.kernel:
        kernel_binary = args.kernel
        if not kernel_binary.exists():
            print(json.dumps({
                "success": False,
                "error": f"Kernel binary not found: {kernel_binary}",
                "error_type": "file_not_found"
            }), file=sys.stderr)
            sys.exit(1)
    else:
        try:
            kernel_binary = find_kernel_binary(args.mode)
        except FileNotFoundError as e:
            print(json.dumps({
                "success": False,
                "error": str(e),
                "error_type": "file_not_found"
            }), file=sys.stderr)
            sys.exit(1)

    if args.debug:
        print(f"Using kernel: {kernel_binary}", file=sys.stderr)

    # Generate session ID
    session_id = f"gdb_{datetime.now().strftime('%Y%m%d_%H%M%S')}"

    # Create and start session
    session = GDBSession(session_id, kernel_binary, args.mode)

    try:
        if args.debug:
            print(f"Starting session {session_id}...", file=sys.stderr)

        result = session.start(timeout=args.timeout)
        result["success"] = True
        print(json.dumps(result, indent=2))
        sys.exit(0)

    except Exception as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "start_failed"
        }), file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
