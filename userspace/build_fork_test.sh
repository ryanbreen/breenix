#!/bin/bash
# Build fork_test.asm into an ELF binary

# Exit on any error
set -e

echo "Building fork_test.asm..."

# Assemble to object file
nasm -f elf64 fork_test.asm -o fork_test.o

# Link into ELF executable with explicit start address
# Using x86_64 Linux toolchain
x86_64-elf-ld -static -nostdlib -Ttext=0x10000000 fork_test.o -o fork_test.elf || \
    x86_64-linux-gnu-ld -static -nostdlib -Ttext=0x10000000 fork_test.o -o fork_test.elf || \
    ld.lld -static -nostdlib -Ttext=0x10000000 fork_test.o -o fork_test.elf

# Show the result
echo "Built fork_test.elf successfully!"
ls -la fork_test.elf

# Display ELF header info
echo ""
echo "ELF Header info:"
readelf -h fork_test.elf | grep -E "Entry point|Type|Machine"

# Display program headers
echo ""
echo "Program headers:"
readelf -l fork_test.elf

# Clean up object file
rm fork_test.o

echo ""
echo "fork_test.elf is ready for use!"