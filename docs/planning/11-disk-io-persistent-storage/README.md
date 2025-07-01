# Disk I/O Planning

This directory contains planning documents for implementing disk I/O and persistent storage in Breenix.

## Current Status
- Phase Status: 📋 PLANNED (High Priority)
- Blocking: Dynamic program loading

## Goals
1. Enable loading programs from disk instead of embedding in kernel
2. Implement basic block device abstraction
3. Support read-only filesystem initially
4. Add write support later

## Planned Implementation Path
1. **RAM Disk** - In-memory filesystem for testing
2. **ATA PIO Driver** - Simple disk access (works in QEMU)
3. **FAT32 Read-Only** - Well-documented, good tooling
4. **Block Cache** - Performance optimization
5. **Write Support** - Full filesystem operations

## Key Design Decisions
- Start with ATA PIO (simpler than AHCI)
- Use FAT32 for initial filesystem (compatibility)
- Block-based interface matching sector sizes
- Async I/O support from the start

## Dependencies
- Memory management (for buffers) ✅
- Interrupt handling (for disk IRQs) ✅
- Process management (for exec from disk) ✅

## Success Criteria
- Can read files from disk image
- Can execute programs stored on disk
- No longer need include_bytes! for userspace programs