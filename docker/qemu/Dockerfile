# Dockerfile for running Breenix tests in isolated QEMU
# Includes tools for keyboard input testing via QEMU monitor

FROM ubuntu:24.04

# Install QEMU and OVMF
RUN apt-get update && apt-get install -y \
    qemu-system-x86 \
    ovmf \
    expect \
    && rm -rf /var/lib/apt/lists/*

# Create working directory
WORKDIR /breenix

# Default command - will be overridden
CMD ["qemu-system-x86_64", "--version"]
