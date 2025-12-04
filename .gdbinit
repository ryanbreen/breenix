# Breenix GDB Configuration
# This file is automatically loaded when GDB starts in the breenix directory

# Set architecture to 64-bit x86_64 with Intel syntax
set architecture i386:x86-64:intel
set disassembly-flavor intel

# Disable pagination (useful for automation and long outputs)
set pagination off

# Show thread events (helpful for understanding context switches)
set print thread-events on

# Connect to QEMU GDB server
define breenix-connect
    target remote localhost:1234
    echo Connected to QEMU GDB server on localhost:1234\n
end

document breenix-connect
Connect to QEMU GDB server on localhost:1234.
Usage: breenix-connect
end

# Show x86_64 segment registers in a readable format
define show-segments
    printf "=== Segment Registers ===\n"
    printf "CS: 0x%04x  (Code Segment)\n", $cs
    printf "DS: 0x%04x  (Data Segment)\n", $ds
    printf "SS: 0x%04x  (Stack Segment)\n", $ss
    printf "ES: 0x%04x  (Extra Segment)\n", $es
    printf "FS: 0x%04x  (FS Segment)\n", $fs
    printf "GS: 0x%04x  (GS Segment)\n", $gs
end

document show-segments
Display x86_64 segment registers (CS, DS, SS, ES, FS, GS).
Usage: show-segments
end

# Show control registers (paging, protection)
define show-control
    printf "=== Control Registers ===\n"
    printf "CR0: 0x%016lx  (Protection Enable, Paging)\n", $cr0
    printf "CR2: 0x%016lx  (Page Fault Address)\n", $cr2
    printf "CR3: 0x%016lx  (Page Directory Base)\n", $cr3
    printf "CR4: 0x%016lx  (PAE, PSE, PGE, etc.)\n", $cr4
    printf "CR8: 0x%016lx  (Task Priority)\n", $cr8
end

document show-control
Display x86_64 control registers (CR0-CR4, CR8).
Usage: show-control
end

# Set common Breenix kernel breakpoints
define breenix-breaks
    echo Setting common Breenix breakpoints...\n
    # Use hardware breakpoint for kernel entry (works before paging)
    hbreak kernel_main
    # Software breakpoints for runtime code
    break rust_syscall_handler
    break timer_interrupt_handler
    break page_fault_handler
    echo Breakpoints set:\n
    echo   - kernel_main (hardware breakpoint)\n
    echo   - rust_syscall_handler\n
    echo   - timer_interrupt_handler\n
    echo   - page_fault_handler\n
end

document breenix-breaks
Set common breakpoints for Breenix kernel debugging:
  - kernel_main (hardware breakpoint for early boot)
  - rust_syscall_handler (syscall entry point)
  - timer_interrupt_handler (APIC timer interrupts)
  - page_fault_handler (page fault handler)
Usage: breenix-breaks
end

# Show full register state (general purpose + control + segments)
define show-all-registers
    echo === General Purpose Registers ===\n
    info registers rax rbx rcx rdx rsi rdi rbp rsp r8 r9 r10 r11 r12 r13 r14 r15 rip rflags
    echo \n
    show-control
    echo \n
    show-segments
end

document show-all-registers
Display all registers: general purpose, control, and segment registers.
Usage: show-all-registers
end

# Show current stack with context
define show-stack
    if $argc == 0
        set $count = 32
    else
        set $count = $arg0
    end
    printf "=== Stack (RSP: 0x%016lx) ===\n", $rsp
    x/$count\gx $rsp
end

document show-stack
Display stack contents starting from RSP.
Usage: show-stack [count]
  count: number of 8-byte values to display (default: 32)
Example: show-stack 16
end

# Examine interrupt descriptor table entry
define show-idt-entry
    if $argc == 0
        printf "Usage: show-idt-entry <vector>\n"
        printf "Example: show-idt-entry 14  # Page fault\n"
    else
        set $vector = $arg0
        # IDT entries are 16 bytes each
        set $idtr = 0
        # Note: Reading IDTR requires architecture-specific code
        printf "IDT Entry %d (0x%02x):\n", $vector, $vector
        printf "  (Use 'info registers idtr' if available)\n"
    end
end

document show-idt-entry
Display information about an IDT (Interrupt Descriptor Table) entry.
Usage: show-idt-entry <vector>
Example: show-idt-entry 14  # Show page fault IDT entry
end

# Show current instruction context
define ctx
    printf "=== Instruction Context ===\n"
    printf "RIP: 0x%016lx\n", $rip
    x/5i $rip
    printf "\n=== Registers ===\n"
    printf "RAX: 0x%016lx  RBX: 0x%016lx  RCX: 0x%016lx  RDX: 0x%016lx\n", $rax, $rbx, $rcx, $rdx
    printf "RSI: 0x%016lx  RDI: 0x%016lx  RBP: 0x%016lx  RSP: 0x%016lx\n", $rsi, $rdi, $rbp, $rsp
    printf "\n=== Stack Top ===\n"
    x/8gx $rsp
end

document ctx
Show execution context: current instruction, registers, and stack.
Usage: ctx
end

# Pretty print startup message
printf "\n"
printf "╔════════════════════════════════════════════════════════════╗\n"
printf "║            Breenix Kernel GDB Configuration                ║\n"
printf "╚════════════════════════════════════════════════════════════╝\n"
printf "\n"
printf "Custom commands available:\n"
printf "  breenix-connect      - Connect to QEMU on localhost:1234\n"
printf "  breenix-breaks       - Set common kernel breakpoints\n"
printf "  show-segments        - Display segment registers\n"
printf "  show-control         - Display control registers\n"
printf "  show-all-registers   - Display all registers\n"
printf "  show-stack [n]       - Display stack (default: 32 entries)\n"
printf "  ctx                  - Show execution context\n"
printf "\n"
printf "Quick start:\n"
printf "  1. In terminal 1: BREENIX_GDB=1 cargo run --release --bin qemu-uefi\n"
printf "  2. In terminal 2: gdb target/x86_64-breenix/release/kernel\n"
printf "  3. (gdb) breenix-connect\n"
printf "  4. (gdb) breenix-breaks\n"
printf "  5. (gdb) c\n"
printf "\n"
