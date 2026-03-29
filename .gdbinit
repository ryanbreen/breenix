# Breenix GDB Configuration
# This file is automatically loaded when GDB starts in the breenix directory
# Supports both x86_64 and ARM64 (aarch64) targets.
# Architecture is auto-detected from the loaded binary.

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

# ===================================================================
# x86_64-specific commands
# ===================================================================

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

# Show x86_64 control registers (paging, protection)
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

# Set common x86_64 Breenix kernel breakpoints
define breenix-breaks-x86
    echo Setting common x86_64 Breenix breakpoints...\n
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

document breenix-breaks-x86
Set common x86_64 breakpoints for Breenix kernel debugging.
Usage: breenix-breaks-x86
end

# Show full x86_64 register state (general purpose + control + segments)
define show-all-registers-x86
    echo === General Purpose Registers ===\n
    info registers rax rbx rcx rdx rsi rdi rbp rsp r8 r9 r10 r11 r12 r13 r14 r15 rip rflags
    echo \n
    show-control
    echo \n
    show-segments
end

document show-all-registers-x86
Display all x86_64 registers: general purpose, control, and segment registers.
Usage: show-all-registers-x86
end

# Show x86_64 stack with context
define show-stack-x86
    if $argc == 0
        set $count = 32
    else
        set $count = $arg0
    end
    printf "=== Stack (RSP: 0x%016lx) ===\n", $rsp
    x/$count\gx $rsp
end

document show-stack-x86
Display x86_64 stack contents starting from RSP.
Usage: show-stack-x86 [count]
  count: number of 8-byte values to display (default: 32)
end

# Examine interrupt descriptor table entry
define show-idt-entry
    if $argc == 0
        printf "Usage: show-idt-entry <vector>\n"
        printf "Example: show-idt-entry 14  # Page fault\n"
    else
        set $vector = $arg0
        printf "IDT Entry %d (0x%02x):\n", $vector, $vector
        printf "  (Use 'info registers idtr' if available)\n"
    end
end

document show-idt-entry
Display information about an x86_64 IDT (Interrupt Descriptor Table) entry.
Usage: show-idt-entry <vector>
Example: show-idt-entry 14  # Show page fault IDT entry
end

# Show x86_64 instruction context
define ctx-x86
    printf "=== Instruction Context ===\n"
    printf "RIP: 0x%016lx\n", $rip
    x/5i $rip
    printf "\n=== Registers ===\n"
    printf "RAX: 0x%016lx  RBX: 0x%016lx  RCX: 0x%016lx  RDX: 0x%016lx\n", $rax, $rbx, $rcx, $rdx
    printf "RSI: 0x%016lx  RDI: 0x%016lx  RBP: 0x%016lx  RSP: 0x%016lx\n", $rsi, $rdi, $rbp, $rsp
    printf "\n=== Stack Top ===\n"
    x/8gx $rsp
end

document ctx-x86
Show x86_64 execution context: current instruction, registers, and stack.
Usage: ctx-x86
end

# ===================================================================
# ARM64 (aarch64)-specific commands
# ===================================================================

# Show ARM64 system registers (exception/MMU state)
define show-sysregs
    printf "=== ARM64 System Registers ===\n"
    printf "ELR_EL1:  0x%016lx  (Exception Link Register)\n", $elr_el1
    printf "SPSR_EL1: 0x%016lx  (Saved Program Status)\n", $spsr_el1
    printf "ESR_EL1:  0x%016lx  (Exception Syndrome)\n", $esr_el1
    printf "FAR_EL1:  0x%016lx  (Fault Address)\n", $far_el1
    printf "SCTLR_EL1: 0x%016lx  (System Control)\n", $sctlr_el1
    printf "TCR_EL1:  0x%016lx  (Translation Control)\n", $tcr_el1
    printf "TTBR0_EL1: 0x%016lx  (Translation Table Base 0)\n", $ttbr0_el1
    printf "TTBR1_EL1: 0x%016lx  (Translation Table Base 1)\n", $ttbr1_el1
    printf "VBAR_EL1: 0x%016lx  (Vector Base Address)\n", $vbar_el1
    printf "DAIF:     0x%016lx  (Interrupt Mask Bits)\n", $daif
end

document show-sysregs
Display ARM64 system registers (ELR, SPSR, ESR, FAR, SCTLR, TCR, TTBR, VBAR, DAIF).
Usage: show-sysregs
end

# Show ARM64 general purpose registers
define show-all-registers-arm64
    echo === ARM64 General Purpose Registers ===\n
    info registers x0 x1 x2 x3 x4 x5 x6 x7
    info registers x8 x9 x10 x11 x12 x13 x14 x15
    info registers x16 x17 x18 x19 x20 x21 x22 x23
    info registers x24 x25 x26 x27 x28 x29 x30 sp pc
    echo \n
    show-sysregs
end

document show-all-registers-arm64
Display all ARM64 registers: x0-x30, sp, pc, and system registers.
Usage: show-all-registers-arm64
end

# Set common ARM64 Breenix kernel breakpoints
define breenix-breaks-arm64
    echo Setting common ARM64 Breenix breakpoints...\n
    hbreak kernel_main
    break handle_sync_exception
    break handle_irq
    echo Breakpoints set:\n
    echo   - kernel_main (hardware breakpoint)\n
    echo   - handle_sync_exception\n
    echo   - handle_irq\n
end

document breenix-breaks-arm64
Set common ARM64 breakpoints for Breenix kernel debugging.
Usage: breenix-breaks-arm64
end

# Show ARM64 stack with context
define show-stack-arm64
    if $argc == 0
        set $count = 32
    else
        set $count = $arg0
    end
    printf "=== Stack (SP: 0x%016lx) ===\n", $sp
    x/$count\gx $sp
end

document show-stack-arm64
Display ARM64 stack contents starting from SP.
Usage: show-stack-arm64 [count]
  count: number of 8-byte values to display (default: 32)
end

# Show ARM64 instruction context
define ctx-arm64
    printf "=== Instruction Context ===\n"
    printf "PC: 0x%016lx\n", $pc
    x/5i $pc
    printf "\n=== Registers ===\n"
    printf "X0:  0x%016lx  X1:  0x%016lx  X2:  0x%016lx  X3:  0x%016lx\n", $x0, $x1, $x2, $x3
    printf "X4:  0x%016lx  X5:  0x%016lx  X6:  0x%016lx  X7:  0x%016lx\n", $x4, $x5, $x6, $x7
    printf "X29: 0x%016lx  X30: 0x%016lx  SP:  0x%016lx\n", $x29, $x30, $sp
    printf "\n=== Stack Top ===\n"
    x/8gx $sp
end

document ctx-arm64
Show ARM64 execution context: current instruction, registers, and stack.
Usage: ctx-arm64
end

# Decode ARM64 ESR_EL1 exception syndrome
define decode-esr
    set $esr = $esr_el1
    set $ec = ($esr >> 26) & 0x3f
    set $iss = $esr & 0x1ffffff
    set $il = ($esr >> 25) & 1
    printf "ESR_EL1: 0x%08lx\n", $esr
    printf "  EC  (Exception Class): 0x%02lx", $ec
    if $ec == 0x15
        printf " (SVC from AArch64)\n"
    end
    if $ec == 0x20
        printf " (Instruction Abort, lower EL)\n"
    end
    if $ec == 0x21
        printf " (Instruction Abort, same EL)\n"
    end
    if $ec == 0x24
        printf " (Data Abort, lower EL)\n"
    end
    if $ec == 0x25
        printf " (Data Abort, same EL)\n"
    end
    if $ec == 0x2c
        printf " (FP/SIMD access)\n"
    end
    if $ec != 0x15 && $ec != 0x20 && $ec != 0x21 && $ec != 0x24 && $ec != 0x25 && $ec != 0x2c
        printf "\n"
    end
    printf "  IL  (Instruction Length): %d (%s)\n", $il, $il ? "32-bit" : "16-bit"
    printf "  ISS (Instruction Specific Syndrome): 0x%07lx\n", $iss
    printf "  FAR_EL1: 0x%016lx\n", $far_el1
end

document decode-esr
Decode ARM64 ESR_EL1 exception syndrome register.
Usage: decode-esr
end

# ===================================================================
# Architecture-neutral aliases (dispatch to correct arch)
# ===================================================================

# Smart breakpoint setter
define breenix-breaks
    # Try ARM64 first (check for x0 register)
    # If we fail, fall back to x86_64
    python
try:
    gdb.execute("show architecture", to_string=True)
    arch = gdb.execute("show architecture", to_string=True)
    if "aarch64" in arch:
        gdb.execute("breenix-breaks-arm64")
    else:
        gdb.execute("breenix-breaks-x86")
except:
    gdb.execute("breenix-breaks-x86")
    end
end

document breenix-breaks
Set common breakpoints for Breenix kernel debugging (auto-detects architecture).
Usage: breenix-breaks
end

# Smart register display
define show-all-registers
    python
try:
    arch = gdb.execute("show architecture", to_string=True)
    if "aarch64" in arch:
        gdb.execute("show-all-registers-arm64")
    else:
        gdb.execute("show-all-registers-x86")
except:
    gdb.execute("show-all-registers-x86")
    end
end

document show-all-registers
Display all registers (auto-detects architecture).
Usage: show-all-registers
end

# Smart stack display
define show-stack
    if $argc == 0
        set $count = 32
    else
        set $count = $arg0
    end
    python
try:
    arch = gdb.execute("show architecture", to_string=True)
    if "aarch64" in arch:
        gdb.execute("show-stack-arm64 " + str(int(gdb.parse_and_eval("$count"))))
    else:
        gdb.execute("show-stack-x86 " + str(int(gdb.parse_and_eval("$count"))))
except:
    gdb.execute("show-stack-x86 " + str(int(gdb.parse_and_eval("$count"))))
    end
end

document show-stack
Display stack contents (auto-detects architecture).
Usage: show-stack [count]
  count: number of 8-byte values to display (default: 32)
end

# Smart execution context
define ctx
    python
try:
    arch = gdb.execute("show architecture", to_string=True)
    if "aarch64" in arch:
        gdb.execute("ctx-arm64")
    else:
        gdb.execute("ctx-x86")
except:
    gdb.execute("ctx-x86")
    end
end

document ctx
Show execution context: current instruction, registers, and stack (auto-detects architecture).
Usage: ctx
end

# Pretty print startup message
printf "\n"
printf "====================================================================\n"
printf "            Breenix Kernel GDB Configuration (multi-arch)\n"
printf "====================================================================\n"
printf "\n"
printf "Architecture-neutral commands (auto-detect x86_64 / aarch64):\n"
printf "  breenix-connect      - Connect to QEMU on localhost:1234\n"
printf "  breenix-breaks       - Set common kernel breakpoints\n"
printf "  show-all-registers   - Display all registers\n"
printf "  show-stack [n]       - Display stack (default: 32 entries)\n"
printf "  ctx                  - Show execution context\n"
printf "\n"
printf "x86_64-specific:\n"
printf "  show-segments        - Display segment registers\n"
printf "  show-control         - Display control registers (CR0-CR4, CR8)\n"
printf "  show-idt-entry <n>   - Examine IDT entry\n"
printf "\n"
printf "ARM64-specific:\n"
printf "  show-sysregs         - Display system registers (ELR, ESR, FAR, ...)\n"
printf "  decode-esr           - Decode ESR_EL1 exception syndrome\n"
printf "\n"
printf "Quick start (x86_64):\n"
printf "  ./breenix-gdb-chat/scripts/gdb_session.sh start\n"
printf "\n"
printf "Quick start (ARM64):\n"
printf "  ./breenix-gdb-chat/scripts/gdb_session.sh start --arch aarch64\n"
printf "\n"
