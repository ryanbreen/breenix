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
| Physical Memory Management | ✅ Frame allocator | ✅ | Both use bootloader memory map, new has 94 MiB usable |
| Virtual Memory (Paging) | ✅ OffsetPageTable | ✅ | Both use OffsetPageTable with physical memory mapping |
| Heap Allocation | ✅ Bump allocator | ✅ | Both have #[global_allocator], new has 1024 KiB heap |
| Stack Overflow Protection | 🚧 Double fault only | ✅ Guard pages + Double fault | New has full guard page implementation with enhanced page fault detection |

### Interrupt Handling
| Feature | Legacy | New | Notes |
|---------|--------|-----|-------|
| IDT (Interrupt Descriptor Table) | ✅ Full implementation | 🚧 Basic | New has breakpoint, double fault, timer, keyboard |
| GDT (Global Descriptor Table) | ✅ | ✅ | New has kernel/user segments, TSS with 8KB stack |
| Exception Handlers | ✅ Many types | ✅ Core handlers | New has divide by zero, breakpoint, invalid opcode, double fault, page fault, generic handler |
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
| Keyboard Driver | ✅ Full async | ✅ Interrupt-driven | New has complete scancode processing |
| Keyboard Events | ✅ Event system | ✅ Event structure | New has KeyEvent with modifiers |
| Scancode Translation | ✅ | ✅ | Both translate scancodes to ASCII |
| Modifier Key Tracking | ✅ All modifiers | ✅ All modifiers | Shift, Ctrl, Alt, Cmd, Caps Lock |
| Caps Lock Handling | ✅ | ✅ | Both correctly handle alphabetic-only caps |
| Special Key Combos | ✅ Ctrl+S, Ctrl+D | ✅ Ctrl+C/D/S | Both support special combinations |

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
| Async Executor | ✅ | ✅ | Both have cooperative multitasking with BTreeMap task storage |
| Task Spawning | ✅ | ✅ | New has Task::new() and executor.spawn() |
| Future Support | ✅ | ✅ | New uses Pin<Box<dyn Future<Output = ()>>> |
| Waker/Wake Support | ✅ | ✅ | Both implement TaskWaker with Arc<ArrayQueue> |
| Async I/O - Keyboard | ✅ ScancodeStream | ✅ ScancodeStream | Both use crossbeam-queue for scancode buffering |
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
| Tests | ✅ Integration tests | ✅ Complete framework | New has 25+ tests using shared QEMU, comprehensive validation |

## Summary

### New Kernel Has
1. Modern framebuffer-based graphics with embedded-graphics
2. Clean, minimal codebase structure
3. Basic interrupt handling (keyboard, timer)
4. Dual logging to both framebuffer and serial port
5. Early boot message buffering (captures pre-serial messages)
6. Comprehensive timer system with RTC integration
7. **Complete integration testing framework (25+ tests with shared QEMU)**
8. GDT with TSS for interrupt handling (8KB double fault stack)
9. **Complete memory management system (frame allocator, paging, heap)**
10. **Physical memory management with 94 MiB usable memory**
11. **1024 KiB heap with bump allocator and #[global_allocator]**
12. **Async executor with cooperative multitasking and Future support**
13. **Guard page stack protection with enhanced page fault detection**

### Legacy Kernel Has (Not in New)
1. ~~Comprehensive memory management (paging, heap)~~ **Now implemented in new kernel**
2. VGA text mode display
3. ~~Async task execution system~~ **Now implemented in new kernel**
4. Network driver infrastructure
5. PCI bus support
6. More complete interrupt handling
7. ~~Complete test infrastructure~~ **Now implemented in new kernel**
8. Event system
9. Interrupt statistics tracking
10. System calls

### Migration Priority Suggestions
Based on typical OS development needs:

1. **High Priority**
   - ~~Memory management (heap allocation, paging)~~ ✅ Complete
   - ~~Serial output (for better debugging)~~ ✅ Complete
   - ~~More exception handlers~~ ✅ Complete
   - ~~GDT setup~~ ✅ Complete

2. **Medium Priority**
   - ~~Async executor for multitasking~~ ✅ Complete
   - ~~Timer configuration and time tracking~~ ✅ Complete
   - ~~Keyboard scancode to ASCII translation~~ ✅ Complete
   - ~~Basic test framework~~ ✅ Complete

3. **Low Priority**
   - Network drivers
   - PCI support
   - VGA text mode (since framebuffer works)
   - System calls (until user-space is needed)