#!/bin/bash
set -e

# Build the userspace test program
echo "Building userspace test program..."

# Build assembly tests first
echo "Building assembly tests..."
nasm -f elf64 -o syscall_test.o syscall_test.asm
rust-lld -flavor gnu -nostdlib -o syscall_test.elf syscall_test.o -e _start

# Build with cargo (config is in .cargo/config.toml)
cargo build --release

# The outputs are already ELF files
cp target/x86_64-breenix/release/hello_time hello_time.elf
cp target/x86_64-breenix/release/hello_world hello_world.elf
cp target/x86_64-breenix/release/counter counter.elf
cp target/x86_64-breenix/release/spinner spinner.elf
cp target/x86_64-breenix/release/fork_test fork_test.elf

# Create flat binaries
rust-objcopy -O binary syscall_test.elf syscall_test.bin
rust-objcopy -O binary hello_time.elf hello_time.bin
rust-objcopy -O binary hello_world.elf hello_world.bin
rust-objcopy -O binary counter.elf counter.bin
rust-objcopy -O binary spinner.elf spinner.bin
rust-objcopy -O binary fork_test.elf fork_test.bin

echo "Built all ELF files"
echo "syscall_test size: $(stat -f%z syscall_test.bin 2>/dev/null || stat -c%s syscall_test.bin) bytes"
echo "hello_time size: $(stat -f%z hello_time.bin 2>/dev/null || stat -c%s hello_time.bin) bytes"
echo "hello_world size: $(stat -f%z hello_world.bin 2>/dev/null || stat -c%s hello_world.bin) bytes"
echo "counter size: $(stat -f%z counter.bin 2>/dev/null || stat -c%s counter.bin) bytes"
echo "spinner size: $(stat -f%z spinner.bin 2>/dev/null || stat -c%s spinner.bin) bytes"
echo "fork_test size: $(stat -f%z fork_test.bin 2>/dev/null || stat -c%s fork_test.bin) bytes"