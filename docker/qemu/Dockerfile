# Dockerfile for running Breenix tests in isolated QEMU
# Includes tools for keyboard input testing via QEMU monitor
# Supports X11 and VNC display modes for interactive use

FROM ubuntu:24.04

# Install QEMU with display support and OVMF
RUN apt-get update && apt-get install -y \
    qemu-system-x86 \
    ovmf \
    expect \
    # X11 and SDL support for graphical display
    libsdl2-2.0-0 \
    libgtk-3-0 \
    libvte-2.91-0 \
    x11-apps \
    && rm -rf /var/lib/apt/lists/*

# Create working directory
WORKDIR /breenix

# Default command - will be overridden
CMD ["qemu-system-x86_64", "--version"]
