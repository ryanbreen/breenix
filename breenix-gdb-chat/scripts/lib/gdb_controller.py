#!/usr/bin/env python3
"""
GDB process controller for Breenix kernel debugging.
Uses file-based I/O for persistent GDB sessions.
"""

import os
import subprocess
import time
import signal
import json
import fcntl
import select
from pathlib import Path
from typing import Optional, Dict, Any, List

try:
    import pexpect
except ImportError:
    pexpect = None

try:
    from .gdb_parser import parse_registers, parse_backtrace, parse_memory, truncate_output
except ImportError:
    from gdb_parser import parse_registers, parse_backtrace, parse_memory, truncate_output


class GDBSession:
    """Manages a GDB debugging session connected to QEMU."""

    SESSION_DIR = Path("/tmp/breenix_gdb_sessions")
    GDB_PROMPT = "(gdb)"

    def __init__(self, session_id: str, kernel_binary: Path, mode: str = "uefi"):
        self.session_id = session_id
        self.kernel_binary = kernel_binary
        self.mode = mode
        self.gdb_process: Optional[subprocess.Popen] = None
        self.qemu_process: Optional[subprocess.Popen] = None
        self.command_count = 0
        self.start_time = time.time()

        self.SESSION_DIR.mkdir(exist_ok=True)

        # Session files
        self.metadata_file = self.SESSION_DIR / f"{session_id}.json"
        self.qemu_log = self.SESSION_DIR / f"{session_id}.qemu.log"
        self.gdb_log = self.SESSION_DIR / f"{session_id}.gdb.log"

    def start(self, timeout: int = 300) -> Dict[str, Any]:
        """Start GDB session and connect to QEMU."""

        # 1. Start QEMU
        self.qemu_process = self._start_qemu()
        time.sleep(3)

        if self.qemu_process.poll() is not None:
            raise RuntimeError("QEMU failed to start")

        # 2. Start GDB with stdin/stdout pipes
        self.gdb_process = self._start_gdb()

        # 3. Wait for GDB prompt
        self._wait_for_prompt(timeout=30)

        # 4. Configure GDB
        self._send_command("set pagination off")
        self._send_command("set confirm off")

        # 5. Connect to QEMU
        output = self._send_command("target remote localhost:1234", timeout=30)
        if "Connection refused" in output or "Error" in output:
            raise ConnectionError(f"Failed to connect to QEMU: {output}")

        # 6. Save metadata
        self._save_metadata()

        return {
            "session_id": self.session_id,
            "gdb_pid": self.gdb_process.pid,
            "qemu_pid": self.qemu_process.pid,
            "status": "connected",
            "kernel_binary": str(self.kernel_binary),
            "mode": self.mode,
        }

    def _start_qemu(self) -> subprocess.Popen:
        """Start QEMU with GDB server enabled."""
        env = os.environ.copy()
        env["BREENIX_GDB"] = "1"

        breenix_dir = Path.home() / "fun/code/breenix"
        cmd = ["cargo", "run", "--bin", "qemu-uefi" if self.mode == "uefi" else "qemu-bios"]
        cmd.extend(["--", "-serial", "stdio", "-display", "none"])

        return subprocess.Popen(
            cmd,
            stdout=open(self.qemu_log, 'w'),
            stderr=subprocess.STDOUT,
            env=env,
            cwd=breenix_dir
        )

    def _start_gdb(self) -> subprocess.Popen:
        """Start GDB with pipes for I/O."""
        breenix_dir = Path.home() / "fun/code/breenix"

        return subprocess.Popen(
            ["gdb", str(self.kernel_binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            cwd=breenix_dir,
            bufsize=0  # Unbuffered
        )

    def _wait_for_prompt(self, timeout: int = 10) -> str:
        """Wait for GDB prompt and return output."""
        output = ""
        deadline = time.time() + timeout

        # Make stdout non-blocking
        fd = self.gdb_process.stdout.fileno()
        fl = fcntl.fcntl(fd, fcntl.F_GETFL)
        fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)

        while time.time() < deadline:
            try:
                ready, _, _ = select.select([self.gdb_process.stdout], [], [], 0.1)
                if ready:
                    chunk = self.gdb_process.stdout.read(4096)
                    if chunk:
                        output += chunk.decode('utf-8', errors='replace')
                        if self.GDB_PROMPT in output:
                            return output
            except:
                pass

        raise TimeoutError(f"Timeout waiting for GDB prompt. Output: {output[:500]}")

    def _send_command(self, command: str, timeout: int = 30) -> str:
        """Send a command to GDB and wait for response."""
        if not self.gdb_process or self.gdb_process.poll() is not None:
            raise RuntimeError("GDB process not running")

        # Send command
        self.gdb_process.stdin.write(f"{command}\n".encode())
        self.gdb_process.stdin.flush()

        # Wait for response
        return self._wait_for_prompt(timeout)

    def execute_command(self, command: str, timeout: int = 30) -> Dict[str, Any]:
        """Execute a GDB command and return parsed output."""
        start = time.time()

        try:
            raw_output = self._send_command(command, timeout)

            # Remove prompt and command echo
            lines = raw_output.split('\n')
            output_lines = []
            for line in lines:
                if command in line or line.strip() == self.GDB_PROMPT:
                    continue
                if line.strip():
                    output_lines.append(line)

            output = '\n'.join(output_lines).strip()
            execution_time = int((time.time() - start) * 1000)
            self.command_count += 1

            # Check for errors
            if "Cannot" in output or "No symbol" in output:
                return {
                    "command": command,
                    "success": False,
                    "error": output,
                    "error_type": "gdb_error",
                    "execution_time_ms": execution_time
                }

            # Parse output
            parsed = self._parse_output(command, output)

            return {
                "command": command,
                "success": True,
                "output": parsed,
                "raw": truncate_output(output),
                "execution_time_ms": execution_time
            }

        except TimeoutError as e:
            return {
                "command": command,
                "success": False,
                "error": str(e),
                "error_type": "timeout"
            }
        except Exception as e:
            return {
                "command": command,
                "success": False,
                "error": str(e),
                "error_type": "unknown"
            }

    def _parse_output(self, command: str, output: str) -> Any:
        """Parse GDB output based on command type."""
        cmd_lower = command.lower().strip()

        if cmd_lower.startswith("info reg"):
            return parse_registers(output)
        elif cmd_lower.startswith("bt") or cmd_lower.startswith("backtrace"):
            return parse_backtrace(output)
        elif cmd_lower.startswith("x/"):
            return parse_memory(output)
        return output

    def stop(self, force: bool = False) -> Dict[str, Any]:
        """Stop session."""
        duration = time.time() - self.start_time

        try:
            if self.gdb_process and self.gdb_process.poll() is None:
                if force:
                    self.gdb_process.kill()
                else:
                    self.gdb_process.stdin.write(b"quit\n")
                    self.gdb_process.stdin.flush()
                    try:
                        self.gdb_process.wait(timeout=5)
                    except:
                        self.gdb_process.kill()

            if self.qemu_process and self.qemu_process.poll() is None:
                self.qemu_process.terminate()
                try:
                    self.qemu_process.wait(timeout=5)
                except:
                    self.qemu_process.kill()

        finally:
            self._cleanup()

        return {
            "session_id": self.session_id,
            "status": "terminated",
            "total_commands": self.command_count,
            "session_duration_s": int(duration)
        }

    def _save_metadata(self):
        """Save session metadata."""
        metadata = {
            "session_id": self.session_id,
            "gdb_pid": self.gdb_process.pid if self.gdb_process else None,
            "qemu_pid": self.qemu_process.pid if self.qemu_process else None,
            "kernel_binary": str(self.kernel_binary),
            "mode": self.mode,
            "start_time": self.start_time,
        }
        with open(self.metadata_file, 'w') as f:
            json.dump(metadata, f, indent=2)

    def _cleanup(self):
        """Clean up session files."""
        for f in self.SESSION_DIR.glob(f"{self.session_id}.*"):
            try:
                f.unlink()
            except:
                pass

    @classmethod
    def list_sessions(cls) -> List[Dict[str, Any]]:
        """List all sessions."""
        sessions = []
        if not cls.SESSION_DIR.exists():
            return sessions

        for metadata_file in cls.SESSION_DIR.glob("*.json"):
            try:
                with open(metadata_file) as f:
                    metadata = json.load(f)

                gdb_alive = False
                qemu_alive = False

                if metadata.get("gdb_pid"):
                    try:
                        os.kill(metadata["gdb_pid"], 0)
                        gdb_alive = True
                    except OSError:
                        pass

                if metadata.get("qemu_pid"):
                    try:
                        os.kill(metadata["qemu_pid"], 0)
                        qemu_alive = True
                    except OSError:
                        pass

                sessions.append({
                    "session_id": metadata["session_id"],
                    "gdb_pid": metadata.get("gdb_pid"),
                    "qemu_pid": metadata.get("qemu_pid"),
                    "gdb_alive": gdb_alive,
                    "qemu_alive": qemu_alive,
                    "mode": metadata.get("mode"),
                })
            except:
                continue

        return sessions


# Global session storage for persistent sessions
_active_sessions: Dict[str, GDBSession] = {}


def get_or_create_session(session_id: str) -> GDBSession:
    """Get existing session or raise error."""
    if session_id in _active_sessions:
        return _active_sessions[session_id]
    raise FileNotFoundError(f"Session {session_id} not found in memory")


def register_session(session: GDBSession):
    """Register a session in the global store."""
    _active_sessions[session.session_id] = session
