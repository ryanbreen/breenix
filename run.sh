#!/bin/bash
# Start Breenix in interactive mode with telnet port forwarding (2323)
# Port 2323 is forwarded automatically by qemu-uefi
#
# Two windows:
#   - QEMU window: Type commands here (PS/2 keyboard -> shell)
#   - This terminal: Watch serial output (debug messages)
cargo run --release --features testing,external_test_bins,interactive --bin qemu-uefi -- -display cocoa -serial mon:stdio "$@"
