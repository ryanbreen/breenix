#!/usr/bin/env python3
"""
Unified GDB chat interface for Breenix kernel debugging.

This script maintains a persistent GDB session and accepts commands via stdin.
Each line of input is a GDB command; output is JSON on stdout.

Usage:
    # Interactive mode (for testing)
    python3 gdb_chat.py

    # Single command mode
    echo "info registers" | python3 gdb_chat.py

    # Multiple commands
    printf "break main\ncontinue\ninfo registers\n" | python3 gdb_chat.py
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

    def __init__(self, kernel_binary: Path, mode: str = "uefi"):
        self.kernel_binary = kernel_binary
        self.mode = mode
        self.gdb_process: Optional[subprocess.Popen] = None
        self.qemu_process: Optional[subprocess.Popen] = None
        self.breenix_dir = Path.home() / "fun/code/breenix"
        self.section_addrs: Dict[str, int] = {}  # ELF section addresses

    def start(self) -> Dict[str, Any]:
        """Start QEMU and GDB, connect them."""
        # Start QEMU
        self.qemu_process = self._start_qemu()
        time.sleep(3)

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

        return {
            "success": True,
            "gdb_pid": self.gdb_process.pid,
            "qemu_pid": self.qemu_process.pid,
            "status": "connected",
            "symbols": f"loaded at base {hex(self.KERNEL_BASE)}",
            "sections": {k: hex(v) for k, v in self.section_addrs.items()}
        }

    def _start_qemu(self) -> subprocess.Popen:
        """Start QEMU with GDB server using debug build for symbol matching."""
        env = os.environ.copy()
        env["BREENIX_GDB"] = "1"

        # Use debug build (--profile dev) to match debug kernel symbols
        # Include testing features to run userspace tests
        cmd = ["cargo", "run", "--profile", "dev", "--features", "testing,external_test_bins", "--bin", f"qemu-{self.mode}"]
        cmd.extend(["--", "-serial", "stdio", "-display", "none"])

        return subprocess.Popen(
            cmd,
            stdin=subprocess.DEVNULL,  # Don't inherit stdin!
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
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

    def execute(self, command: str, timeout: int = None, interrupt_after: int = None) -> Dict[str, Any]:
        """Execute GDB command and return structured result.

        Args:
            command: GDB command to execute
            timeout: Total timeout for waiting for prompt (default varies by command)
            interrupt_after: For continue/run, send Ctrl+C after this many seconds to interrupt execution
        """
        if not self.gdb_process or self.gdb_process.poll() is not None:
            return {"success": False, "error": "GDB not running", "command": command}

        # Auto-detect timeout and interrupt behavior based on command
        cmd_lower = command.strip().lower()
        allow_breakpoint = False

        if timeout is None:
            if cmd_lower in ('continue', 'c', 'cont'):
                timeout = 120  # 2 minutes for continue
                allow_breakpoint = True
                if interrupt_after is None:
                    interrupt_after = 30  # Auto-interrupt after 30s if no breakpoint hit
            elif cmd_lower.startswith('run'):
                timeout = 120  # 2 minutes for run
                allow_breakpoint = True
                if interrupt_after is None:
                    interrupt_after = 30  # Auto-interrupt after 30s if no breakpoint hit
            else:
                timeout = 30  # 30 seconds default

        start = time.time()

        try:
            # If we have interrupt_after, start a timer to send Ctrl+C
            if interrupt_after:
                def send_interrupt():
                    time.sleep(interrupt_after)
                    if self.gdb_process and self.gdb_process.poll() is None:
                        sys.stderr.write(f"[DEBUG] Sending Ctrl+C to GDB after {interrupt_after}s\n")
                        sys.stderr.flush()
                        self.gdb_process.send_signal(signal.SIGINT)

                import threading
                interrupt_thread = threading.Thread(target=send_interrupt, daemon=True)
                interrupt_thread.start()

            raw = self._send_raw(command, timeout, allow_breakpoint)

            # Clean output
            lines = [l for l in raw.split('\n')
                     if l.strip() and l.strip() != self.GDB_PROMPT and command not in l]
            output = '\n'.join(lines).strip()

            # Parse special outputs
            parsed = self._parse(command, output)

            return {
                "success": True,
                "command": command,
                "output": parsed,
                "raw": output[:2000],  # Truncate
                "time_ms": int((time.time() - start) * 1000)
            }

        except TimeoutError:
            return {"success": False, "error": "timeout", "command": command}
        except Exception as e:
            return {"success": False, "error": str(e), "command": command}

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


def find_kernel() -> Path:
    """Find kernel binary."""
    breenix = Path.home() / "fun/code/breenix"

    # Prefer debug build
    debug = breenix / "target/x86_64-breenix/debug/kernel"
    if debug.exists():
        return debug

    release = breenix / "target/x86_64-breenix/release/kernel"
    if release.exists():
        return release

    raise FileNotFoundError("Kernel not found")


def main():
    # Force stdin line buffering
    import io
    stdin_unbuffered = io.TextIOWrapper(
        io.BufferedReader(io.FileIO(0, mode='r', closefd=False)),
        line_buffering=True
    )

    # Find kernel
    try:
        kernel = find_kernel()
    except FileNotFoundError as e:
        print(json.dumps({"success": False, "error": str(e)}))
        sys.exit(1)

    # Create session
    chat = GDBChat(kernel)

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

            result = chat.execute(cmd)
            print(json.dumps(result))
            sys.stdout.flush()

    except EOFError:
        pass
    finally:
        chat.stop()


if __name__ == "__main__":
    main()
