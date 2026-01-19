# Dockerfile for running Breenix tests in isolated QEMU
# This ensures each test run is completely isolated from others

FROM ubuntu:24.04

# Install QEMU and OVMF
RUN apt-get update && apt-get install -y \
    qemu-system-x86 \
    ovmf \
    && rm -rf /var/lib/apt/lists/*

# Create working directory
WORKDIR /breenix

# Default command - will be overridden
CMD ["qemu-system-x86_64", "--version"]
