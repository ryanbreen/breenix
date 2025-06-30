#!/bin/bash
set -e

# Build the userspace test program
echo "Building userspace test program..."

# Build with cargo (config is in .cargo/config.toml)
cargo build --release

# The output is already an ELF file
cp target/x86_64-breenix/release/hello_time hello_time.elf

# Create a flat binary
rust-objcopy -O binary hello_time.elf hello_time.bin

echo "Built hello_time.elf and hello_time.bin"
echo "Size: $(stat -f%z hello_time.bin 2>/dev/null || stat -c%s hello_time.bin) bytes"