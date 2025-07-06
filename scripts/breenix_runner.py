#!/usr/bin/env python3
"""
Breenix runner with command injection capability
"""

import subprocess
import sys
import os
import time
import socket
import select
import threading
from datetime import datetime
import pty
import termios
import tty

class BreenixRunner:
    def __init__(self, mode="uefi", display=False):
        self.mode = mode
        self.display = display
        self.process = None
        self.master_fd = None
        self.log_file = None
        self.log_path = self._create_log_file()
        
    def _create_log_file(self):
        """Create timestamped log file"""
        script_dir = os.path.dirname(os.path.abspath(__file__))
        project_root = os.path.dirname(script_dir)
        logs_dir = os.path.join(project_root, "logs")
        os.makedirs(logs_dir, exist_ok=True)
        
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        log_path = os.path.join(logs_dir, f"breenix_{timestamp}.log")
        
        print(f"Logging to: {log_path}")
        self.log_file = open(log_path, 'w')
        return log_path
        
    def start(self):
        """Start Breenix with PTY for serial interaction"""
        # Create a pseudo-terminal
        self.master_fd, slave_fd = pty.openpty()
        
        # Build the cargo command
        bin_name = f"qemu-{self.mode}"
        cmd = ["cargo", "run", "--release", "--bin", bin_name, "--"]
        
        # Add QEMU arguments
        # Use pty for bidirectional serial communication
        cmd.extend(["-serial", f"pty"])
        if not self.display:
            cmd.extend(["-display", "none"])
            
        print(f"Starting Breenix in {self.mode} mode...")
        
        # Start the process
        self.process = subprocess.Popen(
            cmd,
            pass_fds=(slave_fd,),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            universal_newlines=True
        )
        
        # Close the slave end in parent
        os.close(slave_fd)
        
        # Start threads to handle output
        self._start_output_threads()
        
        # Wait for kernel to initialize
        print("Waiting for kernel to initialize...")
        time.sleep(5)
        
    def _start_output_threads(self):
        """Start threads to handle serial and process output"""
        # Thread to read from PTY and log
        def read_serial():
            while self.process and self.process.poll() is None:
                try:
                    # Check if data is available
                    r, _, _ = select.select([self.master_fd], [], [], 0.1)
                    if r:
                        data = os.read(self.master_fd, 1024).decode('utf-8', errors='ignore')
                        if data:
                            sys.stdout.write(data)
                            sys.stdout.flush()
                            self.log_file.write(data)
                            self.log_file.flush()
                except:
                    break
                    
        # Thread to read process stdout
        def read_stdout():
            while self.process and self.process.poll() is None:
                line = self.process.stdout.readline()
                if line:
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    self.log_file.write(line)
                    self.log_file.flush()
                    
        threading.Thread(target=read_serial, daemon=True).start()
        threading.Thread(target=read_stdout, daemon=True).start()
        
    def send_command(self, command):
        """Send a command to the serial console"""
        if not self.master_fd:
            print("Error: Breenix not running")
            return
            
        print(f"\n>>> Sending command: {command}")
        self.log_file.write(f"\n>>> Sending command: {command}\n")
        self.log_file.flush()
        
        # Add newline if not present
        if not command.endswith('\n'):
            command += '\n'
            
        os.write(self.master_fd, command.encode())
        time.sleep(0.5)  # Give a bit of time for the command to be processed
        
    def send_key(self, key):
        """Send a special key combination (e.g., Ctrl+U)"""
        if not self.master_fd:
            print("Error: Breenix not running")
            return
            
        key_map = {
            'ctrl+u': b'\x15',  # Ctrl+U
            'ctrl+p': b'\x10',  # Ctrl+P
            'ctrl+f': b'\x06',  # Ctrl+F
            'ctrl+e': b'\x05',  # Ctrl+E
            'ctrl+t': b'\x14',  # Ctrl+T
            'ctrl+m': b'\x0d',  # Ctrl+M (Enter)
            'ctrl+c': b'\x03',  # Ctrl+C
        }
        
        key_lower = key.lower()
        if key_lower in key_map:
            print(f"\n>>> Sending key: {key}")
            os.write(self.master_fd, key_map[key_lower])
        else:
            print(f"Unknown key combination: {key}")
            
    def wait(self):
        """Wait for the process to complete"""
        if self.process:
            self.process.wait()
            
    def stop(self):
        """Stop Breenix"""
        if self.process:
            print("\nStopping Breenix...")
            self.process.terminate()
            time.sleep(1)
            if self.process.poll() is None:
                self.process.kill()
                
        if self.master_fd:
            os.close(self.master_fd)
            
        if self.log_file:
            self.log_file.close()
            print(f"\nLog saved to: {self.log_path}")
            
def main():
    """Run Breenix with optional commands"""
    import argparse
    
    parser = argparse.ArgumentParser(description='Run Breenix with command injection')
    parser.add_argument('--mode', choices=['uefi', 'bios'], default='uefi',
                        help='Boot mode (default: uefi)')
    parser.add_argument('--display', action='store_true',
                        help='Show QEMU display window')
    parser.add_argument('--commands', nargs='*',
                        help='Commands to send after boot')
    parser.add_argument('--interactive', action='store_true',
                        help='Stay in interactive mode after commands')
    
    args = parser.parse_args()
    
    runner = BreenixRunner(mode=args.mode, display=args.display)
    
    try:
        runner.start()
        
        # Send any commands
        if args.commands:
            for cmd in args.commands:
                runner.send_command(cmd)
                time.sleep(1)  # Give command time to execute
                
        # Interactive mode or wait
        if args.interactive:
            print("\nEntering interactive mode. Type 'exit' to quit.")
            print("Special keys: ctrl+u, ctrl+p, ctrl+f, ctrl+e, ctrl+t, ctrl+m")
            
            while True:
                try:
                    user_input = input("> ").strip()
                    if user_input.lower() == 'exit':
                        break
                    elif user_input.lower().startswith('ctrl+'):
                        runner.send_key(user_input)
                    else:
                        runner.send_command(user_input)
                except KeyboardInterrupt:
                    break
        else:
            # Just wait for process to complete
            runner.wait()
            
    except KeyboardInterrupt:
        print("\nInterrupted by user")
    finally:
        runner.stop()
        
if __name__ == '__main__':
    main()