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
cp target/x86_64-breenix/release/isolation isolation.elf
cp target/x86_64-breenix/release/isolation_attacker isolation_attacker.elf

# Create flat binaries
rust-objcopy -O binary hello_time.elf hello_time.bin
rust-objcopy -O binary hello_world.elf hello_world.bin
rust-objcopy -O binary counter.elf counter.bin
rust-objcopy -O binary spinner.elf spinner.bin
rust-objcopy -O binary fork_test.elf fork_test.bin
rust-objcopy -O binary isolation.elf isolation.bin
rust-objcopy -O binary isolation_attacker.elf isolation_attacker.bin

echo "Built all ELF files"
echo "hello_time size: $(stat -f%z hello_time.bin 2>/dev/null || stat -c%s hello_time.bin) bytes"
echo "hello_world size: $(stat -f%z hello_world.bin 2>/dev/null || stat -c%s hello_world.bin) bytes"
echo "counter size: $(stat -f%z counter.bin 2>/dev/null || stat -c%s counter.bin) bytes"
echo "spinner size: $(stat -f%z spinner.bin 2>/dev/null || stat -c%s spinner.bin) bytes"
echo "fork_test size: $(stat -f%z fork_test.bin 2>/dev/null || stat -c%s fork_test.bin) bytes"