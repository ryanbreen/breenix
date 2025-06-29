# Breenix Feature Comparison: Legacy vs New Kernel

## Overview
This document compares features between the legacy Breenix kernel (src.legacy/) and the new kernel implementation (kernel/).

## Feature Status Legend
- ✅ Fully implemented
- 🚧 Partially implemented
- ❌ Not implemented
- 🔄 Different implementation approach

## Core Kernel Features

### Memory Management
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Physical Memory Management | ✅ Frame allocator | ❌ | Legacy uses bootloader memory map |
| Virtual Memory (Paging) | ✅ OffsetPageTable | ❌ | Legacy has full page table management |
| Heap Allocation | ✅ Bump allocator | ❌ | Legacy has #[global_allocator] |
| Stack Overflow Protection | ✅ Guard pages | ❌ | |

### Interrupt Handling
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| IDT (Interrupt Descriptor Table) | ✅ Full implementation | 🚧 Basic | New has breakpoint, double fault, timer, keyboard |
| GDT (Global Descriptor Table) | ✅ | ✅ | New has kernel/user segments, TSS with 8KB stack |
| Exception Handlers | ✅ Many types | 🚧 Limited | New only has breakpoint, double fault |
| PIC Support | ✅ | ✅ | Both use pic8259 crate |
| Interrupt Statistics | ✅ | ❌ | Legacy tracks interrupt counts |

## Device Drivers

### Display
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| VGA Text Mode | ~~✅ Full implementation~~ (removed) | ❌ | Legacy code removed after framebuffer completion |
| Framebuffer Graphics | 🚧 Basic | ✅ | New uses embedded-graphics |
| Text Rendering | ✅ VGA hardware | ✅ Software | New renders text to framebuffer |
| Logging | ✅ Serial + VGA | ✅ Framebuffer + Serial | Both outputs, with buffering |

### Input
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Keyboard Driver | ✅ Full async | 🚧 Basic interrupt | New has scancode queue only |
| Keyboard Events | ✅ Event system | ❌ | |
| Scancode Translation | ❌ | ❌ | Neither translates to ASCII |

### Serial Communication
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| UART 16550 Driver | ✅ | ✅ | Both use uart_16550 crate |
| Serial Output | ✅ | ✅ | New has serial_print! macros |
| Debug Printing | ✅ Serial | ✅ Serial + Framebuffer | New outputs to both targets |
| Early Boot Buffering | ❌ | ✅ | New buffers pre-serial messages |

### Timers
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| PIT (Timer Interrupts) | ✅ | ✅ | Both configure PIT for 1000Hz |
| RTC (Real Time Clock) | ✅ | ✅ | Both read Unix timestamp from RTC |
| Monotonic Clock | ✅ | ✅ | Both track ticks since boot |
| Time Tracking | ✅ Boot time, ticks | ✅ | Both track seconds/millis since boot |
| Delay Macro | ✅ | ✅ | Both have delay! macro for busy waits |

### Network
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Intel E1000 Driver | 🚧 Partial | ❌ | |
| RTL8139 Driver | 🚧 Structure only | ❌ | |
| Network Interface Abstraction | ✅ | ❌ | |
| VLAN Support | ✅ | ❌ | |

## System Infrastructure

### Task/Process Management
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Async Executor | ✅ | ❌ | Legacy has cooperative multitasking |
| Task Spawning | ✅ | ❌ | |
| Future Support | ✅ | ❌ | |
| Process Isolation | ❌ | ❌ | Neither has true processes |

### I/O Infrastructure
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Port I/O Abstraction | ✅ Type-safe | 🚧 Direct | New uses x86_64 crate directly |
| PCI Bus Support | ✅ | ❌ | |
| Device Enumeration | ✅ PCI | ❌ | |

### System Calls
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Syscall Infrastructure | 🚧 Mostly commented | ❌ | |
| Time Syscalls | 🚧 | ❌ | |
| Test Syscalls | 🚧 | ❌ | |

## Utilities and Debug Support

### Debug Features
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| Print Macros | ✅ print!, println! | ✅ log macros | 🔄 Different systems |
| Timestamp Support | ✅ | ✅ | Both print Unix timestamps with messages |
| Debug Output Target | ✅ Serial + VGA | ✅ Serial + Framebuffer | |
| Panic Handler | ✅ | ✅ | Both have custom panic handlers |

### Build System
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| UEFI Boot | ✅ | ✅ | |
| BIOS Boot | ✅ | ✅ | |
| Custom Target | ✅ | ✅ | Both use x86_64-breenix.json |
| Tests | ✅ Integration tests | 🚧 Basic | New has serial-based tests |

## Summary

### New Kernel Has
1. Modern framebuffer-based graphics with embedded-graphics
2. Clean, minimal codebase structure
3. Basic interrupt handling (keyboard, timer)
4. Dual logging to both framebuffer and serial port
5. Early boot message buffering (captures pre-serial messages)
6. Comprehensive timer system with RTC integration
7. Serial-based integration testing framework
8. GDT with TSS for interrupt handling (8KB double fault stack)

### Legacy Kernel Has (Not in New)
1. Comprehensive memory management (paging, heap)
2. VGA text mode display
3. Async task execution system
4. Network driver infrastructure
5. PCI bus support
6. More complete interrupt handling
7. Complete test infrastructure
8. Event system
9. Interrupt statistics tracking
10. System calls

### Migration Priority Suggestions
Based on typical OS development needs:

1. **High Priority**
   - Memory management (heap allocation, paging)
   - ~~Serial output (for better debugging)~~ ✅ Complete
   - More exception handlers
   - ~~GDT setup~~ ✅ Complete

2. **Medium Priority**
   - Async executor for multitasking
   - ~~Timer configuration and time tracking~~ ✅ Complete
   - Keyboard scancode to ASCII translation
   - Basic test framework

3. **Low Priority**
   - Network drivers
   - PCI support
   - VGA text mode (since framebuffer works)
   - System calls (until user-space is needed)