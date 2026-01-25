# Dockerfile for running Breenix ARM64 tests in isolated QEMU
# Uses qemu-system-aarch64 for ARM64 kernel testing

FROM ubuntu:24.04

# Install QEMU ARM64 emulator
RUN apt-get update && apt-get install -y \
    qemu-system-arm \
    && rm -rf /var/lib/apt/lists/*

# Create working directory
WORKDIR /breenix

# Default command - will be overridden
CMD ["qemu-system-aarch64", "--version"]
