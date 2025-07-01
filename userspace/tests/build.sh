#!/bin/bash
set -e

# Build the userspace test program
echo "Building userspace test program..."

# Build with cargo (config is in .cargo/config.toml)
cargo build --release

# The outputs are already ELF files
cp target/x86_64-breenix/release/hello_time hello_time.elf
cp target/x86_64-breenix/release/hello_world hello_world.elf

# Create flat binaries
rust-objcopy -O binary hello_time.elf hello_time.bin
rust-objcopy -O binary hello_world.elf hello_world.bin

echo "Built hello_time.elf and hello_world.elf"
echo "hello_time size: $(stat -f%z hello_time.bin 2>/dev/null || stat -c%s hello_time.bin) bytes"
echo "hello_world size: $(stat -f%z hello_world.bin 2>/dev/null || stat -c%s hello_world.bin) bytes"