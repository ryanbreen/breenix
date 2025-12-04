#!/usr/bin/env python3
"""
Stop a GDB debugging session.

Usage:
    stop_session.py --session SESSION_ID
    stop_session.py --session SESSION_ID --force

Output (JSON):
    {
        "session_id": "gdb_20251204_143022",
        "status": "terminated",
        "total_commands": 27,
        "session_duration_s": 183
    }
"""

import argparse
import sys
import json
import os
import signal
from pathlib import Path

# Add lib directory to path
sys.path.insert(0, str(Path(__file__).parent / "lib"))

from gdb_controller import GDBSession


def force_stop_session(session_id: str) -> dict:
    """Force stop a session by killing processes directly."""

    session_dir = Path("/tmp/breenix_gdb_sessions")
    metadata_file = session_dir / f"{session_id}.json"

    if not metadata_file.exists():
        raise FileNotFoundError(f"Session {session_id} not found")

    with open(metadata_file) as f:
        metadata = json.load(f)

    killed = []

    # Kill GDB
    gdb_pid = metadata.get("gdb_pid")
    if gdb_pid:
        try:
            os.kill(gdb_pid, signal.SIGKILL)
            killed.append(f"gdb:{gdb_pid}")
        except OSError:
            pass

    # Kill QEMU
    qemu_pid = metadata.get("qemu_pid")
    if qemu_pid:
        try:
            os.kill(qemu_pid, signal.SIGKILL)
            killed.append(f"qemu:{qemu_pid}")
        except OSError:
            pass

    # Clean up session files
    for f in session_dir.glob(f"{session_id}.*"):
        try:
            f.unlink()
        except:
            pass

    return {
        "session_id": session_id,
        "status": "force_terminated",
        "killed": killed
    }


def main():
    parser = argparse.ArgumentParser(
        description='Stop GDB debugging session'
    )
    parser.add_argument(
        '--session',
        required=True,
        help='Session ID'
    )
    parser.add_argument(
        '--force',
        action='store_true',
        help='Force kill processes without cleanup'
    )

    args = parser.parse_args()

    try:
        if args.force:
            result = force_stop_session(args.session)
        else:
            session = GDBSession.load(args.session)
            result = session.stop()

        result["success"] = True
        print(json.dumps(result, indent=2))
        sys.exit(0)

    except FileNotFoundError as e:
        # Session doesn't exist, try force cleanup anyway
        try:
            result = force_stop_session(args.session)
            result["success"] = True
            result["note"] = "Session metadata missing, performed cleanup"
            print(json.dumps(result, indent=2))
        except:
            print(json.dumps({
                "success": False,
                "error": str(e),
                "error_type": "session_not_found"
            }), file=sys.stderr)
            sys.exit(1)

    except Exception as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "stop_failed"
        }), file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
