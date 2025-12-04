#!/usr/bin/env python3
"""
Unified GDB chat interface for Breenix kernel debugging.

This script maintains a persistent GDB session and accepts commands via stdin.
Each line of input is a GDB command; output is JSON on stdout.

SERIAL OUTPUT CAPTURE:
Serial output from the kernel is captured to /tmp/breenix_gdb_serial.log and
included in JSON responses. This allows agents to see boot stage markers and
kernel print statements alongside GDB debugging output.

Each GDB command response includes a "serial_output" field with any NEW serial
output that appeared during command execution.

Special commands:
    serial      - Get ALL serial output accumulated since session start
    serial-new  - Get only NEW serial output since last read
    quit/exit/q - Terminate session

Usage:
    # Interactive mode (for testing)
    python3 gdb_chat.py

    # Single command mode
    echo "info registers" | python3 gdb_chat.py

    # Multiple commands with serial visibility
    printf "break kernel::kernel_main\\ncontinue\\nserial\\nquit\\n" | python3 gdb_chat.py
"""

import os
import sys
import subprocess
import time
import json
import fcntl
import select
import signal
import re
from pathlib import Path
from typing import Optional, Dict, Any


class GDBChat:
    """Interactive GDB session for Breenix debugging."""

    GDB_PROMPT = "(gdb)"
    # Breenix kernel is loaded at 1 TiB (PIE binary)
    KERNEL_BASE = 0x10000000000
    # Serial output file for capturing kernel print statements
    SERIAL_LOG_FILE = "/tmp/breenix_gdb_serial.log"

    def __init__(self, kernel_binary: Path, mode: str = "uefi", profile: str = "release"):
        self.kernel_binary = kernel_binary
        self.mode = mode
        self.profile = profile  # "release" or "dev" (debug)
        self.gdb_process: Optional[subprocess.Popen] = None
        self.qemu_process: Optional[subprocess.Popen] = None
        self.breenix_dir = Path.home() / "fun/code/breenix"
        self.section_addrs: Dict[str, int] = {}  # ELF section addresses
        self.serial_read_pos: int = 0  # Track how much serial output we've read

    def start(self) -> Dict[str, Any]:
        """Start QEMU and GDB, connect them."""
        # Clean up old serial log file
        try:
            os.remove(self.SERIAL_LOG_FILE)
        except FileNotFoundError:
            pass
        self.serial_read_pos = 0

        # Start QEMU
        self.qemu_process = self._start_qemu()
        # Wait longer for UEFI bootloader to load kernel (8 seconds minimum)
        time.sleep(8)

        if self.qemu_process.poll() is not None:
            return {"success": False, "error": "QEMU failed to start"}

        # Start GDB
        self.gdb_process = subprocess.Popen(
            ["gdb", "-q", str(self.kernel_binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            cwd=self.breenix_dir,
            bufsize=0
        )

        # Wait for prompt
        try:
            self._wait_for_prompt(timeout=30)
        except TimeoutError as e:
            return {"success": False, "error": str(e)}

        # Configure GDB
        self._send_raw("set pagination off")
        self._send_raw("set confirm off")

        # Connect to QEMU
        output = self._send_raw("target remote localhost:1234")
        if "Connection refused" in output:
            return {"success": False, "error": "Cannot connect to QEMU"}

        # Load symbols at correct runtime addresses for PIE kernel
        symbol_output = self._load_symbols_at_runtime_addr()

        # Note: QEMU starts halted at reset vector (0xFFF0) when using -S flag.
        # The bootloader runs when GDB issues 'continue', which loads the kernel.
        # Breakpoints will be hit after the bootloader loads the kernel.

        # Get any initial serial output from UEFI bootloader
        initial_serial = self.get_new_serial_output()

        result = {
            "success": True,
            "gdb_pid": self.gdb_process.pid,
            "qemu_pid": self.qemu_process.pid,
            "status": "connected",
            "symbols": f"loaded at base {hex(self.KERNEL_BASE)}",
            "sections": {k: hex(v) for k, v in self.section_addrs.items()},
            "serial_log_file": self.SERIAL_LOG_FILE
        }

        # Include initial serial output if any
        if initial_serial:
            result["serial_output"] = initial_serial[:4000]

        return result

    def _start_qemu(self) -> subprocess.Popen:
        """Start QEMU with GDB server and serial output to file."""
        env = os.environ.copy()
        env["BREENIX_GDB"] = "1"

        # Build command based on profile
        # Note: Debug builds have timing issues (interrupts fire before init completes)
        # Release builds are recommended for GDB debugging
        if self.profile == "dev":
            cmd = ["cargo", "run", "--profile", "dev", "--features", "testing,external_test_bins", "--bin", f"qemu-{self.mode}"]
        else:
            cmd = ["cargo", "run", "--release", "--features", "testing,external_test_bins", "--bin", f"qemu-{self.mode}"]

        # Serial output goes to file so we can read it and include in JSON responses
        # This allows agents to see boot stage markers and kernel print statements
        cmd.extend(["--", "-serial", f"file:{self.SERIAL_LOG_FILE}", "-display", "none"])

        return subprocess.Popen(
            cmd,
            stdin=subprocess.DEVNULL,  # Don't inherit stdin!
            stdout=subprocess.DEVNULL,  # Cargo build output goes to /dev/null
            stderr=subprocess.DEVNULL,  # Cargo warnings go to /dev/null
            env=env,
            cwd=self.breenix_dir
        )

    def _parse_elf_sections(self) -> Dict[str, int]:
        """Parse ELF section addresses from kernel binary using objdump."""
        sections = {}
        try:
            result = subprocess.run(
                ["objdump", "-h", str(self.kernel_binary)],
                capture_output=True,
                text=True,
                timeout=10
            )
            if result.returncode != 0:
                return sections

            # Parse objdump output:
            # Idx Name          Size      VMA               LMA               File off  Algn
            #   1 .rodata       00007e60  0000000000022a50  0000000000022a50  00022a50  2**4
            for line in result.stdout.split('\n'):
                parts = line.split()
                if len(parts) >= 4:
                    # Check if first part is an index number
                    try:
                        int(parts[0])
                        name = parts[1]
                        vma = int(parts[3], 16)  # Virtual Memory Address
                        if name in ('.text', '.rodata', '.data', '.bss'):
                            sections[name] = vma
                    except (ValueError, IndexError):
                        continue
        except Exception as e:
            sys.stderr.write(f"[WARN] Failed to parse ELF sections: {e}\n")
        return sections

    def _load_symbols_at_runtime_addr(self) -> str:
        """Load symbols with correct offsets for PIE kernel at runtime address."""
        sections = self._parse_elf_sections()
        if not sections or '.text' not in sections:
            return "Failed to parse ELF sections"

        self.section_addrs = sections

        # Calculate runtime addresses: kernel_base + elf_section_addr
        text_addr = self.KERNEL_BASE + sections['.text']

        cmd = f"add-symbol-file {self.kernel_binary} {hex(text_addr)}"

        # Add other sections if available
        for name in ['.rodata', '.data', '.bss']:
            if name in sections:
                runtime_addr = self.KERNEL_BASE + sections[name]
                cmd += f" -s {name} {hex(runtime_addr)}"

        # Execute the command
        output = self._send_raw(cmd)

        # Log what we did
        sys.stderr.write(f"[INFO] Symbol offsets: .text={hex(sections.get('.text', 0))}\n")
        sys.stderr.write(f"[INFO] Runtime addresses: .text={hex(text_addr)}\n")

        return output

    def _wait_for_prompt(self, timeout: int = 10, allow_breakpoint: bool = False) -> str:
        """Wait for GDB prompt or breakpoint hit."""
        output = ""
        deadline = time.time() + timeout
        last_output_time = time.time()
        idle_timeout = 10  # If no output for 10 seconds during continue, something is wrong

        fd = self.gdb_process.stdout.fileno()
        fl = fcntl.fcntl(fd, fcntl.F_GETFL)
        fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)

        while time.time() < deadline:
            ready, _, _ = select.select([self.gdb_process.stdout], [], [], 0.1)
            if ready:
                try:
                    chunk = self.gdb_process.stdout.read(4096)
                    if chunk:
                        decoded = chunk.decode('utf-8', errors='replace')
                        output += decoded
                        last_output_time = time.time()

                        # Debug: Print what we're seeing if we're waiting for breakpoint
                        if allow_breakpoint and len(output) > 100:
                            sys.stderr.write(f"[DEBUG] Received {len(output)} bytes, looking for breakpoint...\n")
                            sys.stderr.flush()

                        # Check for breakpoint hit (higher priority)
                        if allow_breakpoint and ("Breakpoint" in output or "hit Breakpoint" in output):
                            # Look for the prompt after breakpoint message
                            if self.GDB_PROMPT in output:
                                return output

                        # Check for prompt at end of output (not just anywhere)
                        if output.rstrip().endswith(self.GDB_PROMPT):
                            return output
                except:
                    pass

            # If we're waiting for breakpoint and no output for idle_timeout, something's wrong
            if allow_breakpoint and (time.time() - last_output_time) > idle_timeout:
                sys.stderr.write(f"[DEBUG] No output for {idle_timeout}s during continue, still waiting...\n")
                sys.stderr.flush()
                last_output_time = time.time()  # Reset to avoid spam

        raise TimeoutError(f"Timeout waiting for GDB prompt")

    def _send_raw(self, command: str, timeout: int = 30, allow_breakpoint: bool = False) -> str:
        """Send command and return raw output."""
        self.gdb_process.stdin.write(f"{command}\n".encode())
        self.gdb_process.stdin.flush()
        return self._wait_for_prompt(timeout, allow_breakpoint)

    def execute(self, command: str, timeout: int = None) -> Dict[str, Any]:
        """Execute GDB command and return structured result.

        Args:
            command: GDB command to execute
            timeout: Total timeout for waiting for prompt (default varies by command).
                     For interactive debugging, the agent decides the appropriate timeout
                     per command based on what they're expecting.

        NOTE: No automatic interrupt! The agent controls execution by choosing:
        - Short timeouts for commands that should complete quickly
        - Long timeouts (or default) for continue/run waiting for breakpoints
        - The agent can always issue Ctrl+C via a separate mechanism if needed
        """
        if not self.gdb_process or self.gdb_process.poll() is not None:
            return {"success": False, "error": "GDB not running", "command": command}

        # Auto-detect timeout based on command, but NO auto-interrupt
        # The agent decides when to interrupt based on what they learn
        cmd_lower = command.strip().lower()
        allow_breakpoint = False

        if timeout is None:
            if cmd_lower in ('continue', 'c', 'cont'):
                timeout = 300  # 5 minutes for continue - let kernel boot fully
                allow_breakpoint = True
            elif cmd_lower.startswith('run'):
                timeout = 300  # 5 minutes for run
                allow_breakpoint = True
            else:
                timeout = 30  # 30 seconds default for other commands

        start = time.time()

        try:
            raw = self._send_raw(command, timeout, allow_breakpoint)

            # Clean output
            lines = [l for l in raw.split('\n')
                     if l.strip() and l.strip() != self.GDB_PROMPT and command not in l]
            output = '\n'.join(lines).strip()

            # Parse special outputs
            parsed = self._parse(command, output)

            # Get any new serial output that appeared during command execution
            serial_output = self.get_new_serial_output()

            result = {
                "success": True,
                "command": command,
                "output": parsed,
                "raw": output[:2000],  # Truncate
                "time_ms": int((time.time() - start) * 1000)
            }

            # Include serial output if there's any new content
            if serial_output:
                result["serial_output"] = serial_output[:4000]  # Truncate to 4KB

            return result

        except TimeoutError:
            # Still try to get serial output on timeout
            serial_output = self.get_new_serial_output()
            result = {"success": False, "error": "timeout", "command": command}
            if serial_output:
                result["serial_output"] = serial_output[:4000]
            return result
        except Exception as e:
            serial_output = self.get_new_serial_output()
            result = {"success": False, "error": str(e), "command": command}
            if serial_output:
                result["serial_output"] = serial_output[:4000]
            return result

    def _parse(self, command: str, output: str) -> Any:
        """Parse GDB output based on command."""
        cmd = command.lower().strip()

        if cmd.startswith("info reg"):
            return self._parse_registers(output)
        elif cmd.startswith("bt") or cmd.startswith("backtrace"):
            return self._parse_backtrace(output)

        return output

    def _parse_registers(self, output: str) -> Dict[str, str]:
        """Parse register output."""
        regs = {}
        for line in output.split('\n'):
            match = re.match(r'^(\w+)\s+(0x[0-9a-fA-F]+)', line.strip())
            if match:
                regs[match.group(1)] = match.group(2)
        return regs

    def _parse_backtrace(self, output: str) -> list:
        """Parse backtrace output."""
        frames = []
        pattern = r'#(\d+)\s+(0x[0-9a-fA-F]+)\s+in\s+(\S+)'
        for match in re.finditer(pattern, output):
            frames.append({
                "frame": int(match.group(1)),
                "addr": match.group(2),
                "func": match.group(3)
            })
        return frames

    def get_new_serial_output(self, max_bytes: int = 8192) -> str:
        """Read new serial output since last read.

        Returns only the NEW output that hasn't been returned before.
        This allows agents to see kernel print statements incrementally
        as they execute GDB commands.
        """
        try:
            if not os.path.exists(self.SERIAL_LOG_FILE):
                return ""

            with open(self.SERIAL_LOG_FILE, 'rb') as f:
                f.seek(0, 2)  # Seek to end
                file_size = f.tell()

                if file_size <= self.serial_read_pos:
                    return ""  # No new content

                # Read from last position
                f.seek(self.serial_read_pos)
                new_content = f.read(max_bytes)
                self.serial_read_pos = f.tell()

                # Decode as UTF-8, replacing invalid bytes
                return new_content.decode('utf-8', errors='replace')
        except Exception as e:
            return f"[Error reading serial output: {e}]"

    def get_all_serial_output(self) -> str:
        """Read all serial output accumulated so far.

        Useful for getting the complete boot log at any point.
        Does NOT update the read position, so get_new_serial_output()
        will still return incremental output.
        """
        try:
            if not os.path.exists(self.SERIAL_LOG_FILE):
                return ""

            with open(self.SERIAL_LOG_FILE, 'rb') as f:
                content = f.read()
                return content.decode('utf-8', errors='replace')
        except Exception as e:
            return f"[Error reading serial output: {e}]"

    def stop(self):
        """Stop GDB and QEMU."""
        if self.gdb_process and self.gdb_process.poll() is None:
            try:
                self.gdb_process.stdin.write(b"quit\n")
                self.gdb_process.stdin.flush()
                self.gdb_process.wait(timeout=3)
            except:
                self.gdb_process.kill()

        if self.qemu_process and self.qemu_process.poll() is None:
            self.qemu_process.terminate()
            try:
                self.qemu_process.wait(timeout=3)
            except:
                self.qemu_process.kill()


def find_kernel(profile: str = "release") -> Path:
    """Find kernel binary for the specified profile."""
    import glob as glob_mod

    breenix = Path.home() / "fun/code/breenix"

    # First try the symlink location (works for debug builds)
    if profile == "dev":
        symlink_path = breenix / "target/x86_64-breenix/debug/kernel"
        if symlink_path.exists():
            return symlink_path

    # For release builds (and fallback), find the actual kernel artifact
    profile_dir = "debug" if profile == "dev" else "release"
    pattern = str(breenix / f"target/x86_64-unknown-none/{profile_dir}/deps/artifact/kernel-*/bin/kernel-*")
    matches = [p for p in glob_mod.glob(pattern) if not p.endswith('.d')]

    if matches:
        # Return the most recently modified one
        return Path(max(matches, key=lambda p: Path(p).stat().st_mtime))

    raise FileNotFoundError(f"Kernel not found for profile {profile}")


def main():
    import argparse

    parser = argparse.ArgumentParser(description="GDB chat for Breenix kernel")
    parser.add_argument("--profile", choices=["release", "dev"], default="release",
                        help="Build profile (default: release)")
    args = parser.parse_args()

    # Force stdin line buffering
    import io
    stdin_unbuffered = io.TextIOWrapper(
        io.BufferedReader(io.FileIO(0, mode='r', closefd=False)),
        line_buffering=True
    )

    # Find kernel
    try:
        kernel = find_kernel(args.profile)
    except FileNotFoundError as e:
        print(json.dumps({"success": False, "error": str(e)}))
        sys.exit(1)

    # Create session
    chat = GDBChat(kernel, profile=args.profile)

    # Handle Ctrl+C
    def signal_handler(sig, frame):
        chat.stop()
        sys.exit(0)
    signal.signal(signal.SIGINT, signal_handler)

    # Start session
    result = chat.start()
    print(json.dumps(result))
    sys.stdout.flush()

    if not result["success"]:
        sys.exit(1)

    # Read commands from stdin
    try:
        for line in stdin_unbuffered:
            cmd = line.strip()
            if not cmd:
                continue

            if cmd.lower() in ("quit", "exit", "q"):
                chat.stop()
                print(json.dumps({"success": True, "status": "terminated"}))
                break

            # Special commands for serial output
            if cmd.lower() == "serial":
                # Get ALL serial output accumulated so far
                serial_output = chat.get_all_serial_output()
                print(json.dumps({
                    "success": True,
                    "command": "serial",
                    "serial_output": serial_output[:16000]  # 16KB limit for full log
                }))
                sys.stdout.flush()
                continue

            if cmd.lower() == "serial-new":
                # Get only NEW serial output since last read
                serial_output = chat.get_new_serial_output()
                print(json.dumps({
                    "success": True,
                    "command": "serial-new",
                    "serial_output": serial_output[:8000] if serial_output else ""
                }))
                sys.stdout.flush()
                continue

            result = chat.execute(cmd)
            print(json.dumps(result))
            sys.stdout.flush()

    except EOFError:
        pass
    finally:
        chat.stop()


if __name__ == "__main__":
    main()
