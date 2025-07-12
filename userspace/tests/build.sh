#!/bin/bash
set -e

# Build the userspace test program
echo "Building userspace test program..."

# Build with cargo (config is in .cargo/config.toml)
cargo build --release

# The outputs are already ELF files
cp target/x86_64-breenix/release/hello_time hello_time.elf
cp target/x86_64-breenix/release/hello_world hello_world.elf
cp target/x86_64-breenix/release/counter counter.elf
cp target/x86_64-breenix/release/spinner spinner.elf
cp target/x86_64-breenix/release/fork_test fork_test.elf
cp target/x86_64-breenix/release/fork_basic fork_basic.elf
cp target/x86_64-breenix/release/fork_mem_independent fork_mem_independent.elf
cp target/x86_64-breenix/release/fork_deep_stack fork_deep_stack.elf

# Create flat binaries
rust-objcopy -O binary hello_time.elf hello_time.bin
rust-objcopy -O binary hello_world.elf hello_world.bin
rust-objcopy -O binary counter.elf counter.bin
rust-objcopy -O binary spinner.elf spinner.bin
rust-objcopy -O binary fork_test.elf fork_test.bin
rust-objcopy -O binary fork_basic.elf fork_basic.bin
rust-objcopy -O binary fork_mem_independent.elf fork_mem_independent.bin
rust-objcopy -O binary fork_deep_stack.elf fork_deep_stack.bin

echo "Built all ELF files"
echo "hello_time size: $(stat -f%z hello_time.bin 2>/dev/null || stat -c%s hello_time.bin) bytes"
echo "hello_world size: $(stat -f%z hello_world.bin 2>/dev/null || stat -c%s hello_world.bin) bytes"
echo "counter size: $(stat -f%z counter.bin 2>/dev/null || stat -c%s counter.bin) bytes"
echo "spinner size: $(stat -f%z spinner.bin 2>/dev/null || stat -c%s spinner.bin) bytes"
echo "fork_test size: $(stat -f%z fork_test.bin 2>/dev/null || stat -c%s fork_test.bin) bytes"
echo "fork_basic size: $(stat -f%z fork_basic.bin 2>/dev/null || stat -c%s fork_basic.bin) bytes"
echo "fork_mem_independent size: $(stat -f%z fork_mem_independent.bin 2>/dev/null || stat -c%s fork_mem_independent.bin) bytes"
echo "fork_deep_stack size: $(stat -f%z fork_deep_stack.bin 2>/dev/null || stat -c%s fork_deep_stack.bin) bytes"