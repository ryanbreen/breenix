#!/usr/bin/env python3
"""
Debug automation script for the ARM64 ELR=0 crash.

Launches QEMU with serial to file and monitor on a UNIX socket.
Waits for the shell prompt (breenix>), then injects keyboard characters
via QEMU monitor to trigger the crash. Captures and reports diagnostics.

Usage: python3 scripts/debug_elr0_crash.py
"""

import os
import socket
import subprocess
import sys
import time

BREENIX_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KERNEL = os.path.join(BREENIX_ROOT, "target/aarch64-breenix/release/kernel-aarch64")
EXT2_DISK = os.path.join(BREENIX_ROOT, "target/ext2-aarch64.img")
MONITOR_SOCK = "/tmp/breenix_debug_monitor.sock"
OUTPUT_DIR = "/tmp/breenix_debug_elr0"

def cleanup():
    """Kill any leftover QEMU processes and remove socket."""
    os.system("pkill -9 -f 'qemu-system-aarch64.*breenix_debug' 2>/dev/null")
    try:
        os.unlink(MONITOR_SOCK)
    except FileNotFoundError:
        pass

def main():
    cleanup()
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    serial_output = os.path.join(OUTPUT_DIR, "serial_output.txt")

    # Create writable copy of ext2 disk
    ext2_writable = os.path.join(OUTPUT_DIR, "ext2-writable.img")
    os.system(f"cp {EXT2_DISK} {ext2_writable}")

    if not os.path.exists(KERNEL):
        print(f"ERROR: Kernel not found: {KERNEL}")
        sys.exit(1)

    print(f"=== ARM64 ELR=0 Crash Debug Session ===")
    print(f"Kernel: {KERNEL}")
    print(f"Serial output: {serial_output}")
    print()

    # Start QEMU with serial to file and monitor on UNIX socket
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
    ]

    print("Starting QEMU...")
    qemu_proc = subprocess.Popen(
        qemu_cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    # Phase 1: Wait for boot to complete (look for "breenix>" in serial output)
    print("\nPhase 1: Waiting for boot to complete...")
    boot_timeout = 30
    start_time = time.time()
    boot_complete = False

    while time.time() - start_time < boot_timeout:
        if qemu_proc.poll() is not None:
            print("\nERROR: QEMU exited during boot")
            break

        if os.path.exists(serial_output):
            with open(serial_output, "rb") as f:
                data = f.read()
            if b"breenix>" in data:
                boot_complete = True
                print(f"Boot complete! ({len(data)} bytes of serial output)")
                break
            if b"KERNEL PANIC" in data or b"panic!" in data:
                print("KERNEL PANIC detected during boot!")
                break

        time.sleep(1)

    if not boot_complete:
        print("\nERROR: Boot did not complete within timeout")
        if os.path.exists(serial_output):
            with open(serial_output, "rb") as f:
                data = f.read()
            print(f"Serial output ({len(data)} bytes):")
            text = data.decode('utf-8', errors='replace')
            for line in text.split('\n')[-20:]:
                print(f"  {line}")
        qemu_proc.kill()
        sys.exit(1)

    # Phase 2: Wait for system to stabilize
    print("\nPhase 2: Waiting 2s for system to stabilize...")
    time.sleep(2)

    # Phase 3: Connect to QEMU monitor and inject keyboard input
    print("Phase 3: Connecting to QEMU monitor...")
    mon = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for attempt in range(10):
        try:
            mon.connect(MONITOR_SOCK)
            break
        except (ConnectionRefusedError, FileNotFoundError):
            if attempt == 9:
                print("ERROR: Could not connect to QEMU monitor")
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

    # Map characters to QEMU key names
    key_map = {
        'h': 'h', 'e': 'e', 'l': 'l', 'p': 'p',
        '\r': 'ret', '\n': 'ret',
    }

    test_input = "help\r"
    print(f"Phase 3: Injecting keyboard input via monitor: {test_input!r}")

    for ch in test_input:
        key_name = key_map.get(ch, ch)
        cmd = f"sendkey {key_name}\n"
        mon.send(cmd.encode())
        time.sleep(0.1)
        try:
            mon.recv(4096)  # Read response
        except socket.timeout:
            pass

    # Phase 4: Wait and capture output
    print("Phase 4: Capturing output (10s)...")
    time.sleep(10)

    # Read final serial output
    with open(serial_output, "rb") as f:
        all_output = f.read()

    # Cleanup
    print("\n=== Cleaning up ===")
    mon.close()
    qemu_proc.kill()
    qemu_proc.wait()
    cleanup()

    # Summary
    output_text = all_output.decode('utf-8', errors='replace')
    print(f"\n{'='*60}")
    print(f"DEBUG SESSION COMPLETE")
    print(f"{'='*60}")
    print(f"Full output saved to: {serial_output}")
    print(f"Output size: {len(all_output)} bytes")

    crash_detected = (b"INSTRUCTION_ABORT" in all_output or
                      b"!!! FATAL: frame.elr=0" in all_output)

    if crash_detected:
        print(f"\nCRASH DETECTED!")
        # Extract diagnostics
        print("\n--- Context Switch Trace (last entries) ---")
        cs_lines = [l for l in output_text.split('\n') if l.strip().startswith('CS:')]
        for line in cs_lines[-20:]:
            print(f"  {line.strip()}")

        print("\n--- ELR=0 Diagnostic ---")
        for line in output_text.split('\n'):
            if 'FATAL' in line or 'DIAG' in line or 'INSTRUCTION_ABORT' in line:
                print(f"  {line.strip()}")
    else:
        print("\nNo crash detected! The fix appears to work.")
        lines = output_text.strip().split('\n')
        print("\n--- Last 30 lines ---")
        for line in lines[-30:]:
            print(f"  {line}")

    sys.exit(1 if crash_detected else 0)

if __name__ == "__main__":
    main()
