#!/usr/bin/env python3
"""
List active GDB debugging sessions.

Usage:
    list_sessions.py
    list_sessions.py --format text

Output (JSON):
    [
        {
            "session_id": "gdb_20251204_143022",
            "gdb_pid": 12345,
            "qemu_pid": 12346,
            "gdb_alive": true,
            "qemu_alive": true,
            "mode": "uefi"
        }
    ]
"""

import argparse
import sys
import json
from pathlib import Path
from datetime import datetime

# Add lib directory to path
sys.path.insert(0, str(Path(__file__).parent / "lib"))

from gdb_controller import GDBSession


def main():
    parser = argparse.ArgumentParser(
        description='List active GDB debugging sessions'
    )
    parser.add_argument(
        '--format',
        choices=['json', 'text'],
        default='json',
        help='Output format (default: json)'
    )
    parser.add_argument(
        '--cleanup',
        action='store_true',
        help='Remove dead sessions'
    )

    args = parser.parse_args()

    sessions = GDBSession.list_sessions()

    if args.cleanup:
        # Remove sessions where both processes are dead
        session_dir = Path("/tmp/breenix_gdb_sessions")
        cleaned = 0

        for session in sessions:
            if not session["gdb_alive"] and not session["qemu_alive"]:
                # Remove session files
                for f in session_dir.glob(f"{session['session_id']}.*"):
                    try:
                        f.unlink()
                        cleaned += 1
                    except:
                        pass

        # Re-fetch sessions
        sessions = GDBSession.list_sessions()

        if args.format == 'json':
            print(json.dumps({
                "sessions": sessions,
                "cleaned_files": cleaned
            }, indent=2))
        else:
            print(f"Cleaned {cleaned} files from dead sessions")

    else:
        if args.format == 'json':
            print(json.dumps(sessions, indent=2))
        else:
            if not sessions:
                print("No active sessions")
            else:
                print(f"{'Session ID':<30} {'GDB':<8} {'QEMU':<8} {'Mode':<6}")
                print("-" * 60)
                for s in sessions:
                    gdb_status = "alive" if s["gdb_alive"] else "dead"
                    qemu_status = "alive" if s["qemu_alive"] else "dead"
                    print(f"{s['session_id']:<30} {gdb_status:<8} {qemu_status:<8} {s.get('mode', 'uefi'):<6}")


if __name__ == '__main__':
    main()
