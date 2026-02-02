#!/usr/bin/env python3
"""
ARM64 Userspace Test Runner for Breenix

This script boots the ARM64 kernel, waits for the shell prompt,
sends a test command, and captures/parses the output.
"""

import subprocess
import sys
import os
import time
import select
import signal
import threading
import queue

def run_test(test_name, timeout=45):
    """Run a single test and return (success, output)"""
    
    script_dir = os.path.dirname(os.path.abspath(__file__))
    breenix_root = os.path.dirname(os.path.dirname(script_dir))
    
    kernel = os.path.join(breenix_root, "target/aarch64-breenix/release/kernel-aarch64")
    ext2_disk = os.path.join(breenix_root, "target/ext2-aarch64.img")
    
    if not os.path.exists(kernel):
        return False, f"Kernel not found: {kernel}"
    if not os.path.exists(ext2_disk):
        return False, f"ext2 disk not found: {ext2_disk}"
    
    cmd = [
        "qemu-system-aarch64",
        "-M", "virt",
        "-cpu", "cortex-a72",
        "-m", "512",
        "-kernel", kernel,
        "-display", "none",
        "-no-reboot",
        "-device", "virtio-gpu-device",
        "-device", "virtio-keyboard-device",
        "-device", "virtio-blk-device,drive=ext2",
        "-drive", f"if=none,id=ext2,format=raw,readonly=on,file={ext2_disk}",
        "-serial", "stdio"
    ]
    
    output_lines = []
    
    try:
        # Start QEMU
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            bufsize=1,
            universal_newlines=True
        )
        
        # Create output reader thread
        output_queue = queue.Queue()
        def read_output():
            try:
                for line in iter(proc.stdout.readline, ''):
                    output_queue.put(line)
                    if not line:
                        break
            except:
                pass
        
        reader_thread = threading.Thread(target=read_output, daemon=True)
        reader_thread.start()
        
        # Wait for shell prompt
        prompt_found = False
        start_time = time.time()
        
        while time.time() - start_time < timeout:
            try:
                line = output_queue.get(timeout=0.5)
                output_lines.append(line)
                if "breenix>" in line:
                    prompt_found = True
                    break
            except queue.Empty:
                pass
        
        if not prompt_found:
            proc.kill()
            return False, "Timeout waiting for shell prompt\n" + "".join(output_lines)
        
        # Send test command
        proc.stdin.write(test_name + "\n")
        proc.stdin.flush()
        
        # Wait for test to complete (look for result markers or timeout)
        test_start = time.time()
        test_complete = False
        
        while time.time() - test_start < 20:
            try:
                line = output_queue.get(timeout=0.5)
                output_lines.append(line)
                
                # Check for test completion
                if any(marker in line for marker in ["PASS", "FAIL", "Test Summary", ": OK", "panic"]):
                    # Read a bit more output
                    time.sleep(1)
                    while not output_queue.empty():
                        try:
                            output_lines.append(output_queue.get_nowait())
                        except:
                            break
                    test_complete = True
                    break
                    
            except queue.Empty:
                pass
        
        proc.kill()
        proc.wait()
        
    except Exception as e:
        return False, f"Error: {e}"
    
    full_output = "".join(output_lines)
    
    # Determine result
    if "FAIL" in full_output and "failed: 0" not in full_output.lower():
        return False, full_output
    elif "PASS" in full_output or ": OK" in full_output or "passed: " in full_output.lower():
        return True, full_output
    elif "panic" in full_output.lower():
        return False, full_output
    else:
        return None, full_output  # Unknown result

def main():
    if len(sys.argv) < 2:
        print("Usage: run-aarch64-test-runner.py <test_name> [test_name2 ...]")
        print("Example: run-aarch64-test-runner.py clock_gettime_test fork_test")
        sys.exit(1)
    
    tests = sys.argv[1:]
    results = []
    
    for test_name in tests:
        print(f"\n{'='*60}")
        print(f"Running: {test_name}")
        print('='*60)
        
        success, output = run_test(test_name)
        results.append((test_name, success, output))
        
        # Print last 50 lines of output
        lines = output.split('\n')
        print("\n--- Output (last 50 lines) ---")
        for line in lines[-50:]:
            print(line)
        print("--- End Output ---")
        
        if success is True:
            print(f"\nRESULT: PASS")
        elif success is False:
            print(f"\nRESULT: FAIL")
        else:
            print(f"\nRESULT: UNKNOWN")
    
    # Summary
    print(f"\n{'='*60}")
    print("SUMMARY")
    print('='*60)
    passed = sum(1 for _, s, _ in results if s is True)
    failed = sum(1 for _, s, _ in results if s is False)
    unknown = sum(1 for _, s, _ in results if s is None)
    
    print(f"Passed:  {passed}")
    print(f"Failed:  {failed}")
    print(f"Unknown: {unknown}")
    
    if failed > 0:
        sys.exit(1)
    elif unknown > 0:
        sys.exit(2)
    else:
        sys.exit(0)

if __name__ == "__main__":
    main()
