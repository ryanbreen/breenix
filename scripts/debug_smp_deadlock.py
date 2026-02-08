#!/usr/bin/env python3
"""
SMP Deadlock Debug Script for ARM64 Breenix.

Launches QEMU with 4 CPUs, waits for the shell prompt, injects keyboard
commands to trigger a deadlock, then captures all CPU register states
via the QEMU monitor. Optionally resolves PCs to kernel symbols.

Usage: python3 scripts/debug_smp_deadlock.py
"""

import os
import shutil
import socket
import subprocess
import sys
import time

BREENIX_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KERNEL = os.path.join(BREENIX_ROOT, "target/aarch64-breenix/release/kernel-aarch64")
EXT2_DISK = os.path.join(BREENIX_ROOT, "target/ext2-aarch64.img")
MONITOR_SOCK = "/tmp/breenix_smp_deadlock/monitor.sock"
OUTPUT_DIR = "/tmp/breenix_smp_deadlock"

# llvm-nm for symbol resolution (Homebrew LLVM on macOS)
LLVM_NM = "/opt/homebrew/opt/llvm/bin/llvm-nm"

# QEMU sendkey names for printable characters
SENDKEY_MAP = {
    'a': 'a', 'b': 'b', 'c': 'c', 'd': 'd', 'e': 'e', 'f': 'f',
    'g': 'g', 'h': 'h', 'i': 'i', 'j': 'j', 'k': 'k', 'l': 'l',
    'm': 'm', 'n': 'n', 'o': 'o', 'p': 'p', 'q': 'q', 'r': 'r',
    's': 's', 't': 't', 'u': 'u', 'v': 'v', 'w': 'w', 'x': 'x',
    'y': 'y', 'z': 'z',
    '0': '0', '1': '1', '2': '2', '3': '3', '4': '4',
    '5': '5', '6': '6', '7': '7', '8': '8', '9': '9',
    ' ': 'spc', '/': 'slash', '.': 'dot', '-': 'minus',
    '\r': 'ret', '\n': 'ret',
}


def cleanup():
    """Kill leftover QEMU processes and remove socket."""
    os.system("pkill -9 -f 'qemu-system-aarch64.*breenix_smp_deadlock' 2>/dev/null")
    try:
        os.unlink(MONITOR_SOCK)
    except FileNotFoundError:
        pass


def load_symbols():
    """Load kernel symbols from ELF for PC resolution."""
    if not os.path.exists(LLVM_NM):
        print(f"  (llvm-nm not found at {LLVM_NM}, skipping symbol resolution)")
        return []

    try:
        result = subprocess.run(
            [LLVM_NM, "--demangle", KERNEL],
            capture_output=True, text=True, timeout=10
        )
        symbols = []
        for line in result.stdout.splitlines():
            parts = line.strip().split(None, 2)
            if len(parts) >= 3:
                try:
                    addr = int(parts[0], 16)
                    sym_type = parts[1]
                    name = parts[2]
                    if sym_type in ('t', 'T', 'W'):  # text symbols
                        symbols.append((addr, name))
                except ValueError:
                    pass
        symbols.sort(key=lambda x: x[0])
        print(f"  Loaded {len(symbols)} text symbols from kernel ELF")
        return symbols
    except Exception as e:
        print(f"  Warning: Could not load symbols: {e}")
        return []


def resolve_pc(pc, symbols):
    """Resolve a PC value to nearest symbol name."""
    if not symbols:
        return None
    # Binary search for nearest symbol <= pc
    lo, hi = 0, len(symbols) - 1
    best = None
    while lo <= hi:
        mid = (lo + hi) // 2
        if symbols[mid][0] <= pc:
            best = mid
            lo = mid + 1
        else:
            hi = mid - 1
    if best is not None:
        addr, name = symbols[best]
        offset = pc - addr
        if offset < 0x10000:  # reasonable offset
            return f"{name}+0x{offset:x}"
    return None


def monitor_cmd(sock, cmd, timeout=2.0):
    """Send a command to the QEMU monitor and read the response."""
    sock.send((cmd + "\n").encode())
    time.sleep(0.3)

    response = b""
    sock.settimeout(timeout)
    try:
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                break
            response += chunk
            # Check if we got the (qemu) prompt back
            if b"(qemu)" in response:
                break
    except socket.timeout:
        pass

    return response.decode('utf-8', errors='replace')


def inject_string(sock, text, delay=0.15):
    """Inject a string as keyboard input via QEMU monitor sendkey."""
    for ch in text:
        key_name = SENDKEY_MAP.get(ch)
        if key_name is None:
            print(f"    Warning: no sendkey mapping for {ch!r}, skipping")
            continue
        monitor_cmd(sock, f"sendkey {key_name}", timeout=0.5)
        time.sleep(delay)


def wait_for_serial(serial_path, marker, timeout=30, poll_interval=1.0):
    """Wait for a marker string to appear in the serial output file."""
    start = time.time()
    last_size = 0
    while time.time() - start < timeout:
        if os.path.exists(serial_path):
            with open(serial_path, "rb") as f:
                data = f.read()
            if marker.encode() in data:
                return True, data
            if b"KERNEL PANIC" in data or b"panic!" in data:
                return False, data
            cur_size = len(data)
            if cur_size != last_size:
                last_size = cur_size
        time.sleep(poll_interval)
    # Return current data even on timeout
    if os.path.exists(serial_path):
        with open(serial_path, "rb") as f:
            return False, f.read()
    return False, b""


def detect_freeze(serial_path, freeze_time=5.0, check_interval=1.0):
    """Detect a freeze by watching for no new serial output."""
    if not os.path.exists(serial_path):
        return True
    with open(serial_path, "rb") as f:
        initial_size = len(f.read())
    last_size = initial_size
    last_change = time.time()

    while time.time() - last_change < freeze_time:
        time.sleep(check_interval)
        with open(serial_path, "rb") as f:
            cur_size = len(f.read())
        if cur_size != last_size:
            last_size = cur_size
            last_change = time.time()

    return last_size == initial_size or (time.time() - last_change >= freeze_time)


def dump_all_cpus(sock, num_cpus=4):
    """Dump register state for all CPUs via QEMU monitor."""
    results = {}

    # First, get the CPU status overview
    info_cpus = monitor_cmd(sock, "info cpus", timeout=3)
    results["info_cpus"] = info_cpus

    for cpu_id in range(num_cpus):
        # Switch to this CPU
        monitor_cmd(sock, f"cpu {cpu_id}", timeout=2)
        time.sleep(0.2)

        # Get full register dump
        reg_dump = monitor_cmd(sock, "info registers", timeout=3)
        results[f"cpu_{cpu_id}"] = reg_dump

    return results


def extract_pc_from_registers(reg_text):
    """Extract the PC value from QEMU 'info registers' output."""
    for line in reg_text.splitlines():
        line = line.strip()
        # QEMU aarch64 format: "PC=ffff000040097abc"
        if line.startswith("PC=") or " PC=" in line:
            parts = line.split("PC=")
            if len(parts) >= 2:
                hex_str = parts[-1].strip().split()[0]
                try:
                    return int(hex_str, 16)
                except ValueError:
                    pass
        # Alternative format: " pc 0xffff..."
        if line.lower().startswith("pc") or " pc " in line.lower():
            for part in line.split():
                if part.startswith("0x") or part.startswith("0X"):
                    try:
                        return int(part, 16)
                    except ValueError:
                        pass
    return None


def extract_key_registers(reg_text):
    """Extract key registers (PC, SP, LR, PSTATE, ELR) from register dump."""
    regs = {}
    for line in reg_text.splitlines():
        line = line.strip()
        # Parse "REG=value" or "REG = value" format
        for reg_name in ['PC', 'SP', 'LR', 'PSTATE', 'ELR_EL1', 'ESR_EL1',
                         'FAR_EL1', 'SPSR_EL1', 'X0', 'X1', 'X2', 'X3',
                         'X29', 'X30']:
            # Try "REG=hex" format
            marker = f"{reg_name}="
            if marker in line:
                idx = line.index(marker) + len(marker)
                val_str = line[idx:].strip().split()[0]
                try:
                    regs[reg_name] = int(val_str, 16)
                except ValueError:
                    pass
    return regs


def main():
    cleanup()
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    serial_output = os.path.join(OUTPUT_DIR, "serial_output.txt")

    # Pre-checks
    if not os.path.exists(KERNEL):
        print(f"ERROR: Kernel not found: {KERNEL}")
        print("Build with:")
        print("  cargo build --release --features boot_tests --target aarch64-breenix.json \\")
        print("    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \\")
        print("    -p kernel --bin kernel-aarch64")
        sys.exit(1)

    if not os.path.exists(EXT2_DISK):
        print(f"ERROR: ext2 disk not found: {EXT2_DISK}")
        sys.exit(1)

    # Create writable copy of ext2 disk
    ext2_writable = os.path.join(OUTPUT_DIR, "ext2-writable.img")
    shutil.copy2(EXT2_DISK, ext2_writable)

    print("=" * 70)
    print("ARM64 SMP Deadlock Debug Session")
    print("=" * 70)
    print(f"  Kernel:       {KERNEL}")
    print(f"  Ext2 disk:    {EXT2_DISK}")
    print(f"  Serial out:   {serial_output}")
    print(f"  Monitor sock: {MONITOR_SOCK}")
    print(f"  Output dir:   {OUTPUT_DIR}")
    print()

    # Load symbols for PC resolution
    print("Loading kernel symbols...")
    symbols = load_symbols()

    # Remove any stale serial output
    if os.path.exists(serial_output):
        os.unlink(serial_output)

    # Start QEMU
    qemu_cmd = [
        "qemu-system-aarch64",
        "-M", "virt", "-cpu", "cortex-a72", "-m", "512", "-smp", "4",
        "-kernel", KERNEL,
        "-display", "none", "-no-reboot",
        "-device", "virtio-gpu-device",
        "-device", "virtio-keyboard-device",
        "-device", "virtio-blk-device,drive=ext2",
        "-drive", f"if=none,id=ext2,format=raw,file={ext2_writable}",
        "-device", "virtio-net-device,netdev=net0",
        "-netdev", "user,id=net0",
        "-serial", f"file:{serial_output}",
        "-monitor", f"unix:{MONITOR_SOCK},server,nowait",
        "-gdb", "tcp::1234",
    ]

    print("Starting QEMU (4 CPUs, GDB on :1234)...")
    qemu_proc = subprocess.Popen(
        qemu_cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    print(f"  QEMU PID: {qemu_proc.pid}")

    try:
        # Phase 1: Wait for boot
        print("\n[Phase 1] Waiting for boot (looking for 'breenix>')...")
        found, boot_data = wait_for_serial(serial_output, "breenix>", timeout=30)

        if qemu_proc.poll() is not None:
            print("  ERROR: QEMU exited during boot!")
            sys.exit(1)

        if not found:
            print("  ERROR: Boot did not complete within 30s")
            text = boot_data.decode('utf-8', errors='replace')
            print("  Last 20 lines:")
            for line in text.split('\n')[-20:]:
                print(f"    {line}")
            qemu_proc.kill()
            sys.exit(1)

        print(f"  Boot complete! ({len(boot_data)} bytes of serial output)")

        # Phase 2: Stabilize
        print("\n[Phase 2] Waiting 3s for system to stabilize...")
        time.sleep(3)

        # Phase 3: Connect to monitor
        print("\n[Phase 3] Connecting to QEMU monitor...")
        mon = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for attempt in range(10):
            try:
                mon.connect(MONITOR_SOCK)
                break
            except (ConnectionRefusedError, FileNotFoundError):
                if attempt == 9:
                    print("  ERROR: Could not connect to QEMU monitor")
                    qemu_proc.kill()
                    sys.exit(1)
                time.sleep(0.5)

        mon.setblocking(True)
        mon.settimeout(5)

        # Read initial monitor prompt
        try:
            mon.recv(4096)
        except socket.timeout:
            pass

        print("  Connected to monitor.")

        # Phase 4: Inject "help" command (should work)
        print("\n[Phase 4] Injecting 'help' command...")
        inject_string(mon, "help\r")

        print("  Waiting 5s for response...")
        time.sleep(5)

        with open(serial_output, "rb") as f:
            after_help = f.read()
        help_text = after_help.decode('utf-8', errors='replace')
        if "Available commands" in help_text or "help" in help_text.split("breenix>")[-1]:
            print("  'help' command responded successfully.")
        else:
            print("  Warning: 'help' response not clearly detected")

        # Record serial size before trigger command
        pre_trigger_size = len(after_help)

        # Phase 5: Inject trigger command
        trigger_cmd = "cat /proc/cpuinfo"
        print(f"\n[Phase 5] Injecting trigger command: '{trigger_cmd}'...")
        inject_string(mon, trigger_cmd + "\r")

        print("  Waiting up to 15s, watching for freeze...")

        # Check for new output or freeze
        freeze_detected = False
        for check in range(15):
            time.sleep(1)
            with open(serial_output, "rb") as f:
                cur_data = f.read()
            cur_size = len(cur_data)

            if cur_size > pre_trigger_size:
                new_text = cur_data[pre_trigger_size:].decode('utf-8', errors='replace')
                if "breenix>" in new_text:
                    print(f"  Command completed normally after {check+1}s")
                    print("  No deadlock triggered with this command.")
                    # Try another command
                    pre_trigger_size = cur_size
                    trigger_cmd = "ls /proc"
                    print(f"\n  Trying alternate trigger: '{trigger_cmd}'...")
                    inject_string(mon, trigger_cmd + "\r")
                    continue

            # Check for stall (no new output for 5s)
            if check >= 5 and cur_size == pre_trigger_size:
                print(f"  FREEZE DETECTED after {check+1}s - no new serial output!")
                freeze_detected = True
                break
            elif check >= 5:
                # Output grew but no prompt - might be partial freeze
                with open(serial_output, "rb") as f:
                    latest = f.read()
                # Wait 3 more seconds to confirm
                time.sleep(3)
                with open(serial_output, "rb") as f:
                    latest2 = f.read()
                if len(latest2) == len(latest):
                    print(f"  FREEZE DETECTED - output stopped mid-response!")
                    freeze_detected = True
                    break

        if not freeze_detected:
            # Try one more command to be thorough
            print("\n  First commands didn't deadlock. Trying 'echo test'...")
            with open(serial_output, "rb") as f:
                pre_trigger_size = len(f.read())
            inject_string(mon, "echo test\r")
            time.sleep(8)
            with open(serial_output, "rb") as f:
                final_data = f.read()
            if len(final_data) == pre_trigger_size:
                print("  FREEZE DETECTED on 'echo test'!")
                freeze_detected = True
            else:
                print("  No deadlock detected. System appears responsive.")
                # Still dump CPU state for analysis
                print("  Will dump CPU state anyway for baseline.")

        # Phase 6: Stop QEMU and dump CPU state
        print(f"\n[Phase 6] Dumping all CPU register states via QEMU monitor...")

        # First, stop all CPUs
        print("  Sending 'stop' to pause QEMU...")
        monitor_cmd(mon, "stop", timeout=2)
        time.sleep(0.5)

        cpu_dumps = dump_all_cpus(mon, num_cpus=4)

        # Phase 7: Report
        print("\n" + "=" * 70)
        print("CPU STATE DUMP" + (" - DEADLOCK DETECTED" if freeze_detected else " - NO DEADLOCK"))
        print("=" * 70)

        # Print info cpus summary
        print("\n--- info cpus ---")
        print(cpu_dumps.get("info_cpus", "(no data)").strip())

        # Print per-CPU details
        for cpu_id in range(4):
            cpu_key = f"cpu_{cpu_id}"
            reg_text = cpu_dumps.get(cpu_key, "")

            print(f"\n{'='*60}")
            print(f"CPU {cpu_id}")
            print(f"{'='*60}")

            # Extract key registers
            key_regs = extract_key_registers(reg_text)
            pc = key_regs.get('PC')

            if pc is not None:
                sym = resolve_pc(pc, symbols)
                pc_str = f"0x{pc:016x}"
                if sym:
                    pc_str += f"  ({sym})"
                print(f"  PC     = {pc_str}")
            else:
                print(f"  PC     = (could not extract)")

            for rname in ['SP', 'LR', 'PSTATE', 'ELR_EL1', 'ESR_EL1', 'FAR_EL1']:
                val = key_regs.get(rname)
                if val is not None:
                    line = f"  {rname:8s} = 0x{val:016x}"
                    if rname == 'LR':
                        sym = resolve_pc(val, symbols)
                        if sym:
                            line += f"  ({sym})"
                    print(line)

            # Print first few general-purpose registers
            for rname in ['X0', 'X1', 'X2', 'X3', 'X29', 'X30']:
                val = key_regs.get(rname)
                if val is not None:
                    line = f"  {rname:8s} = 0x{val:016x}"
                    if rname == 'X30':  # X30 is LR
                        sym = resolve_pc(val, symbols)
                        if sym:
                            line += f"  ({sym})"
                    print(line)

            # Print full register dump (truncated)
            full_lines = reg_text.strip().splitlines()
            if len(full_lines) > 5:
                print(f"\n  [Full dump: {len(full_lines)} lines]")

        # Save full dumps to files
        dump_file = os.path.join(OUTPUT_DIR, "cpu_dumps.txt")
        with open(dump_file, "w") as f:
            f.write(f"SMP Deadlock Debug - {time.strftime('%Y-%m-%d %H:%M:%S')}\n")
            f.write(f"Freeze detected: {freeze_detected}\n")
            f.write("=" * 70 + "\n\n")

            f.write("--- info cpus ---\n")
            f.write(cpu_dumps.get("info_cpus", "") + "\n\n")

            for cpu_id in range(4):
                f.write(f"=== CPU {cpu_id} ===\n")
                f.write(cpu_dumps.get(f"cpu_{cpu_id}", "(no data)") + "\n\n")

        print(f"\n  Full CPU dumps saved to: {dump_file}")

        # Save serial output
        with open(serial_output, "rb") as f:
            final_serial = f.read()
        final_text = final_serial.decode('utf-8', errors='replace')

        print(f"  Serial output saved to: {serial_output}")
        print(f"  Serial output size: {len(final_serial)} bytes")

        # Print last 30 lines of serial for context
        print("\n--- Last 30 lines of serial output ---")
        for line in final_text.strip().split('\n')[-30:]:
            print(f"  {line}")

        # Summary
        print("\n" + "=" * 70)
        if freeze_detected:
            print("SUMMARY: DEADLOCK DETECTED")
            print()
            print("All CPU program counters above show where each CPU was stuck.")
            print("Look for CPUs spinning in lock acquisition (spin_loop, try_lock)")
            print("or waiting on I/O. If multiple CPUs hold different locks and")
            print("wait for each other, that confirms a classic deadlock.")
            print()
            print("To investigate further with GDB:")
            print("  1. Re-run this script (it starts QEMU with -gdb tcp::1234)")
            print("  2. When freeze is detected, connect GDB:")
            print(f"     gdb {KERNEL}")
            print("     (gdb) set architecture aarch64")
            print("     (gdb) target remote :1234")
            print("     (gdb) thread apply all bt")
        else:
            print("SUMMARY: No deadlock detected in this run")
            print("The system remained responsive to all injected commands.")
            print("Try running again or using different trigger commands.")

        print("=" * 70)

    finally:
        # Cleanup
        try:
            mon.close()
        except Exception:
            pass
        if qemu_proc.poll() is None:
            qemu_proc.kill()
            qemu_proc.wait()
        cleanup()


if __name__ == "__main__":
    main()
