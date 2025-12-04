#!/usr/bin/env python3
"""
Execute GDB command in an active session.

Usage:
    gdb_cmd.py --session SESSION_ID --command "info registers"
    gdb_cmd.py --session SESSION_ID --command "break main" --command "continue"

Output (JSON):
    {
        "command": "info registers",
        "success": true,
        "output": {"rax": "0x0", ...},
        "raw": "rax            0x0    0\n...",
        "execution_time_ms": 45
    }
"""

import argparse
import sys
import json
import os
from pathlib import Path

# Add lib directory to path
sys.path.insert(0, str(Path(__file__).parent / "lib"))

from gdb_controller import GDBSession


def load_session(session_id: str) -> GDBSession:
    """Load an existing session."""

    session_dir = Path("/tmp/breenix_gdb_sessions")
    metadata_file = session_dir / f"{session_id}.json"

    if not metadata_file.exists():
        raise FileNotFoundError(f"Session {session_id} not found")

    with open(metadata_file) as f:
        metadata = json.load(f)

    # Check if processes are still running
    gdb_pid = metadata.get("gdb_pid")
    qemu_pid = metadata.get("qemu_pid")

    if gdb_pid:
        try:
            os.kill(gdb_pid, 0)
        except OSError:
            raise RuntimeError(f"GDB process {gdb_pid} is not running")

    if qemu_pid:
        try:
            os.kill(qemu_pid, 0)
        except OSError:
            raise RuntimeError(f"QEMU process {qemu_pid} is not running")

    # Create session object and reattach
    return GDBSession.load(session_id)


def main():
    parser = argparse.ArgumentParser(
        description='Execute GDB command in active session'
    )
    parser.add_argument(
        '--session',
        required=True,
        help='Session ID'
    )
    parser.add_argument(
        '--command', '-c',
        required=True,
        action='append',
        help='GDB command to execute (can specify multiple)'
    )
    parser.add_argument(
        '--timeout',
        type=int,
        default=30,
        help='Command timeout in seconds (default: 30)'
    )
    parser.add_argument(
        '--format',
        choices=['json', 'text'],
        default='json',
        help='Output format (default: json)'
    )

    args = parser.parse_args()

    try:
        session = load_session(args.session)

        results = []
        for cmd in args.command:
            result = session.execute_command(cmd, timeout=args.timeout)
            results.append(result)

        # Output results
        if args.format == 'json':
            if len(results) == 1:
                print(json.dumps(results[0], indent=2))
            else:
                print(json.dumps(results, indent=2))
        else:
            for result in results:
                if result['success']:
                    print(result.get('raw', result.get('output', '')))
                else:
                    print(f"Error: {result['error']}", file=sys.stderr)

        # Exit with error if any command failed
        if any(not r['success'] for r in results):
            sys.exit(1)

    except FileNotFoundError as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "session_not_found"
        }), file=sys.stderr)
        sys.exit(1)

    except RuntimeError as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "session_dead"
        }), file=sys.stderr)
        sys.exit(1)

    except Exception as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "execution_failed"
        }), file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
