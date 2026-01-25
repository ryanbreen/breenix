# ARM64 Port Handoff Document

## Project Overview

This document provides comprehensive instructions for porting Breenix to ARM64 (AArch64) architecture. The goal is to enable native development on Apple Silicon Macs via Parallels Desktop, while maintaining x86-64 compatibility for GitHub Actions CI.

**Branch:** `feature/arm64-port`
**Worktree:** `/Users/wrb/fun/code/breenix-arm64`
**Parent Branch:** `feature/signal-completion` (at commit `a9e2614`)

## Goals

1. **Dual-architecture support** - Both x86-64 and ARM64 build from the same codebase
2. **Local ARM64 development** - Native Parallels virtualization on Apple Silicon
3. **CI cross-compilation** - GitHub Actions builds both architectures
4. **Feature parity** - ARM64 eventually matches x86-64 functionality

## Current HAL Status

The kernel already has a well-designed Hardware Abstraction Layer in `kernel/src/arch_impl/`:

### Existing Traits (All 9 must be implemented for ARM64)

| Trait | Purpose | x86-64 | ARM64 Equivalent |
|-------|---------|--------|------------------|
| `PrivilegeLevel` | CPU privilege levels | Ring 0/3 | EL0/EL1 |
| `InterruptFrame` | Saved CPU state on exception | IDT frame | Exception frame |
| `PageFlags` | Page table entry flags | PTE flags | Stage 1 descriptors |
| `PageTableOps` | Page table manipulation | CR3, 4-level | TTBR0/1, 4-level |
| `PerCpuOps` | Per-CPU data access | GS segment | TPIDR_EL1 |
| `SyscallFrame` | Syscall arguments | RAX, RDI... | X8, X0-X5 |
| `TimerOps` | High-resolution timer | TSC | Generic timer |
| `InterruptController` | IRQ management | PIC/APIC | GIC |
| `CpuOps` | Basic CPU control | CLI/STI | MSR DAIF |

### Files Location

```
kernel/src/arch_impl/
├── mod.rs           # Architecture selection (modify for #[cfg])
├── traits.rs        # Architecture-agnostic traits (DO NOT MODIFY)
├── x86_64/          # Existing x86-64 implementation
│   ├── mod.rs
│   ├── constants.rs
│   ├── cpu.rs
│   ├── interrupt_frame.rs
│   ├── paging.rs
│   ├── percpu.rs
│   ├── pic.rs
│   ├── privilege.rs
│   └── timer.rs
└── aarch64/         # TO BE CREATED
    ├── mod.rs
    ├── constants.rs
    ├── cpu.rs
    ├── exception_frame.rs
    ├── paging.rs
    ├── percpu.rs
    ├── gic.rs
    ├── privilege.rs
    └── timer.rs
```

---

## Architecture Differences: x86-64 vs ARM64

### Privilege Levels

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| Ring 0 (kernel) | EL1 | Exception Level 1 |
| Ring 3 (user) | EL0 | Exception Level 0 |
| N/A | EL2 | Hypervisor (we don't use) |
| N/A | EL3 | Secure monitor (we don't use) |

### Interrupts and Exceptions

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| IDT (Interrupt Descriptor Table) | Exception Vector Table | At VBAR_EL1 |
| 256 vectors | 16 entries x 4 types | sync/irq/fiq/serror × el0/el1 |
| `iret` | `eret` | Exception return |
| Push to stack | Save to SPSRs + ELRs | Automatic by hardware |

**ARM64 Exception Vector Table Layout:**
```
Offset    Type        Source
0x000     Synchronous  Current EL with SP_EL0
0x080     IRQ          Current EL with SP_EL0
0x100     FIQ          Current EL with SP_EL0
0x180     SError       Current EL with SP_EL0
0x200     Synchronous  Current EL with SP_ELx
0x280     IRQ          Current EL with SP_ELx
0x300     FIQ          Current EL with SP_ELx
0x380     SError       Current EL with SP_ELx
0x400     Synchronous  Lower EL using AArch64
0x480     IRQ          Lower EL using AArch64
0x500     FIQ          Lower EL using AArch64
0x580     SError       Lower EL using AArch64
0x600     Synchronous  Lower EL using AArch32
0x680     IRQ          Lower EL using AArch32
0x700     FIQ          Lower EL using AArch32
0x780     SError       Lower EL using AArch32
```

### Syscalls

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| `syscall` instruction | `svc #0` | Supervisor call |
| RAX = syscall number | X8 = syscall number | |
| RDI, RSI, RDX, R10, R8, R9 | X0, X1, X2, X3, X4, X5 | Arguments |
| RAX = return value | X0 = return value | |

### Page Tables

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| CR3 | TTBR0_EL1 / TTBR1_EL1 | Two bases (user/kernel) |
| 4-level (PML4→PDPT→PD→PT) | 4-level (L0→L1→L2→L3) | Same structure |
| 4KB pages | 4KB pages (also 16KB, 64KB) | We'll use 4KB |
| NX bit (bit 63) | UXN/PXN bits | Separate user/kernel NX |

**ARM64 Page Descriptor Bits (4KB granule, Stage 1):**
```
Bits     Name        Description
[0]      Valid       1 = valid entry
[1]      Table/Block 1 = table (next level), 0 = block (at L0-L2)
[4:2]    AttrIndx    Memory attribute index (MAIR_EL1)
[5]      NS          Non-secure (we don't use)
[6]      AP[1]       Access Permission (0=RW, 1=RO)
[7]      AP[2]       EL0 access (0=no, 1=yes)
[8]      SH[0]       Shareability
[9]      SH[1]       Shareability
[10]     AF          Access Flag (must be 1)
[11]     nG          not Global
[47:12]  Address     Physical address
[50]     GP          Guarded Page (BTI)
[51]     DBM         Dirty Bit Modifier
[52]     Contiguous  Contiguous hint
[53]     PXN         Privileged Execute Never
[54]     UXN/XN      User Execute Never
```

### Timer

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| TSC (RDTSC) | CNTVCT_EL0 | Virtual counter |
| APIC timer | CNTV_CTL_EL0 | Virtual timer control |
| PIT (8254) | N/A | No legacy PIT |
| HPET | N/A | No HPET |

**ARM64 Generic Timer Registers:**
- `CNTFRQ_EL0` - Counter frequency (read-only, set by firmware)
- `CNTVCT_EL0` - Virtual counter value
- `CNTV_CTL_EL0` - Virtual timer control (enable, mask, status)
- `CNTV_CVAL_EL0` - Virtual timer compare value
- `CNTV_TVAL_EL0` - Virtual timer value (countdown)

### Serial I/O

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| 16550 UART (I/O ports 0x3F8) | PL011 UART (MMIO) | Completely different |
| Port-based I/O (in/out) | Memory-mapped | Different access method |

**PL011 UART Registers (base at 0x09000000 for QEMU virt):**
```
Offset  Name    Description
0x000   UARTDR  Data Register
0x018   UARTFR  Flag Register
0x024   UARTIBRD Integer Baud Rate
0x028   UARTFBRD Fractional Baud Rate
0x02C   UARTLCR_H Line Control
0x030   UARTCR  Control Register
0x038   UARTIMSC Interrupt Mask
```

### Interrupt Controller

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| 8259 PIC | GIC (Generic Interrupt Controller) | Different registers |
| APIC (advanced) | GICv2 or GICv3 | We'll use GICv2 |

**GICv2 Components:**
- **GICD** (Distributor) - Routes interrupts to CPUs
- **GICC** (CPU Interface) - Per-CPU interrupt interface

**GICv2 Key Registers:**
```
GICD (Distributor) at 0x08000000 for QEMU virt:
0x000   GICD_CTLR      Distributor Control
0x004   GICD_TYPER     Interrupt Controller Type
0x100   GICD_ISENABLERn Interrupt Set-Enable
0x180   GICD_ICENABLERn Interrupt Clear-Enable
0x400   GICD_IPRIORITYRn Interrupt Priority
0x800   GICD_ITARGETSRn Interrupt Processor Targets
0xC00   GICD_ICFGRn    Interrupt Configuration

GICC (CPU Interface) at 0x08010000 for QEMU virt:
0x000   GICC_CTLR      CPU Interface Control
0x004   GICC_PMR       Interrupt Priority Mask
0x00C   GICC_IAR       Interrupt Acknowledge
0x010   GICC_EOIR      End of Interrupt
```

### Per-CPU Data

| x86-64 | ARM64 | Notes |
|--------|-------|-------|
| GS segment base | TPIDR_EL1 | System register |
| `swapgs` on entry | No swap needed | Single register |
| MSR-based access | MRS/MSR instructions | |

---

## Implementation Phases

### Phase 1: Build Infrastructure

**Goal:** Both architectures compile (ARM64 stubs that panic)

**Tasks:**

1. **Update `kernel/src/arch_impl/mod.rs`:**
   ```rust
   #[cfg(target_arch = "x86_64")]
   pub mod x86_64;
   #[cfg(target_arch = "x86_64")]
   pub use x86_64 as current;

   #[cfg(target_arch = "aarch64")]
   pub mod aarch64;
   #[cfg(target_arch = "aarch64")]
   pub use aarch64 as current;

   pub mod traits;
   pub use traits::*;
   ```

2. **Create `kernel/src/arch_impl/aarch64/mod.rs`:**
   - Export all trait implementations
   - Re-export constants

3. **Create stub implementations for all 9 traits:**
   - Each function body: `unimplemented!("ARM64 not yet implemented")`
   - This allows the kernel to compile

4. **Update `kernel/Cargo.toml`:**
   ```toml
   [target.'cfg(target_arch = "x86_64")'.dependencies]
   x86_64 = "0.15"

   [target.'cfg(target_arch = "aarch64")'.dependencies]
   aarch64-cpu = "9.4"
   tock-registers = "0.8"
   ```

5. **Create `.cargo/config.toml` for ARM64:**
   ```toml
   [build]
   # Default to x86_64 (CI compatible)
   target = "x86_64-breenix.json"

   [target.aarch64-breenix]
   rustflags = ["-C", "link-arg=-Tkernel/src/arch_impl/aarch64/linker.ld"]
   ```

6. **Create `kernel/src/arch_impl/aarch64/linker.ld`:**
   - Define memory layout for ARM64 UEFI boot
   - Entry point, sections, alignment

7. **Update GitHub Actions workflow:**
   ```yaml
   jobs:
     build-x86_64:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
         - run: cargo build --target x86_64-breenix.json

     build-aarch64:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
         - run: cargo build --target aarch64-breenix.json
   ```

**Verification:**
```bash
# In /Users/wrb/fun/code/breenix-arm64
cargo build --target aarch64-breenix.json -Z build-std=core,alloc --release
```

---

### Phase 2: Serial Output (PL011 UART)

**Goal:** Boot to "Hello from ARM64!" on serial console

**Tasks:**

1. **Create `kernel/src/drivers/pl011.rs`:**
   ```rust
   const PL011_BASE: u64 = 0x0900_0000; // QEMU virt machine

   pub fn init() {
       // Enable UART, set 8N1, etc.
   }

   pub fn write_byte(byte: u8) {
       unsafe {
           let dr = PL011_BASE as *mut u32;
           core::ptr::write_volatile(dr, byte as u32);
       }
   }
   ```

2. **Create `kernel/src/arch_impl/aarch64/boot.rs`:**
   - Minimal boot stub that calls PL011 init
   - Print "Hello from ARM64!"

3. **Implement `CpuOps` trait:**
   ```rust
   impl CpuOps for Aarch64Cpu {
       unsafe fn enable_interrupts() {
           asm!("msr daifclr, #2"); // Clear IRQ mask
       }
       unsafe fn disable_interrupts() {
           asm!("msr daifset, #2"); // Set IRQ mask
       }
       fn interrupts_enabled() -> bool {
           let daif: u64;
           unsafe { asm!("mrs {}, daif", out(reg) daif) };
           (daif & (1 << 7)) == 0 // I bit clear = IRQs enabled
       }
       fn halt() {
           unsafe { asm!("wfi") }; // Wait For Interrupt
       }
       fn halt_with_interrupts() {
           unsafe {
               asm!("msr daifclr, #2"); // Enable IRQs
               asm!("wfi");
           }
       }
   }
   ```

**Verification:**
```bash
# Test in QEMU (not Parallels yet - simpler for debugging)
qemu-system-aarch64 -M virt -cpu cortex-a72 -nographic \
    -kernel target/aarch64-breenix/release/kernel.elf
```

---

### Phase 3: Exception Handling

**Goal:** Handle exceptions and interrupts

**Tasks:**

1. **Create exception vector table (`exception_vectors.S`):**
   ```asm
   .section .text.vectors
   .align 11  // 2048-byte aligned

   .global exception_vectors
   exception_vectors:
       // Current EL with SP_EL0
       .align 7
       b sync_current_el_sp0
       .align 7
       b irq_current_el_sp0
       // ... repeat for all 16 entries
   ```

2. **Implement exception handlers:**
   - Synchronous exceptions (syscalls, page faults)
   - IRQ handling (timer, devices)
   - Save/restore full register state

3. **Set VBAR_EL1:**
   ```rust
   unsafe {
       asm!("msr vbar_el1, {}", in(reg) &exception_vectors as *const _ as u64);
   }
   ```

4. **Implement `InterruptFrame` trait:**
   - Define `Aarch64ExceptionFrame` struct
   - Map X0-X30, SP, PC, PSTATE

---

### Phase 4: Memory Management

**Goal:** Virtual memory with 4-level page tables

**Tasks:**

1. **Implement `PageTableOps` trait:**
   ```rust
   impl PageTableOps for Aarch64PageTableOps {
       type Flags = Aarch64PageFlags;

       fn read_root() -> u64 {
           let ttbr0: u64;
           unsafe { asm!("mrs {}, ttbr0_el1", out(reg) ttbr0) };
           ttbr0
       }

       unsafe fn write_root(addr: u64) {
           asm!("msr ttbr0_el1, {}", in(reg) addr);
           asm!("isb"); // Instruction synchronization barrier
       }

       fn flush_tlb_page(addr: u64) {
           unsafe {
               asm!("tlbi vaae1is, {}", in(reg) addr >> 12);
               asm!("dsb sy");
               asm!("isb");
           }
       }

       fn flush_tlb_all() {
           unsafe {
               asm!("tlbi vmalle1is");
               asm!("dsb sy");
               asm!("isb");
           }
       }

       const PAGE_LEVELS: usize = 4;
       const PAGE_SIZE: usize = 4096;
       const ENTRIES_PER_TABLE: usize = 512;
   }
   ```

2. **Implement `PageFlags` trait:**
   - Map Valid, AF, AP[2:1], UXN, PXN, SH, AttrIndx

3. **Configure MAIR_EL1:**
   ```rust
   // Memory Attribute Indirection Register
   // Attr0 = Device-nGnRnE (MMIO)
   // Attr1 = Normal, Inner/Outer Write-Back
   const MAIR_VALUE: u64 = 0x00_44_ff_00;
   ```

4. **Enable MMU:**
   ```rust
   unsafe {
       asm!("msr mair_el1, {}", in(reg) MAIR_VALUE);
       asm!("msr tcr_el1, {}", in(reg) tcr_value);
       asm!("msr ttbr0_el1, {}", in(reg) page_table_addr);
       asm!("isb");

       let mut sctlr: u64;
       asm!("mrs {}, sctlr_el1", out(reg) sctlr);
       sctlr |= 1; // Enable MMU
       asm!("msr sctlr_el1, {}", in(reg) sctlr);
       asm!("isb");
   }
   ```

---

### Phase 5: Timer and Interrupts

**Goal:** Working timer interrupts for scheduling

**Tasks:**

1. **Implement `TimerOps` trait:**
   ```rust
   impl TimerOps for Aarch64Timer {
       fn read_timestamp() -> u64 {
           let cnt: u64;
           unsafe { asm!("mrs {}, cntvct_el0", out(reg) cnt) };
           cnt
       }

       fn frequency_hz() -> Option<u64> {
           let freq: u64;
           unsafe { asm!("mrs {}, cntfrq_el0", out(reg) freq) };
           Some(freq)
       }

       fn ticks_to_nanos(ticks: u64) -> u64 {
           let freq = Self::frequency_hz().unwrap_or(1);
           ticks * 1_000_000_000 / freq
       }
   }
   ```

2. **Set up timer interrupt:**
   ```rust
   pub fn arm_timer(ticks: u64) {
       unsafe {
           asm!("msr cntv_tval_el0, {}", in(reg) ticks);
           asm!("msr cntv_ctl_el0, {}", in(reg) 1u64); // Enable
       }
   }
   ```

3. **Implement `InterruptController` trait (GICv2):**
   ```rust
   impl InterruptController for Gicv2 {
       fn init() {
           // Enable distributor
           // Enable CPU interface
           // Set priority mask
       }

       fn enable_irq(irq: u8) {
           let reg = irq / 32;
           let bit = irq % 32;
           // Write to GICD_ISENABLERn
       }

       fn send_eoi(vector: u8) {
           // Write to GICC_EOIR
       }

       fn irq_offset() -> u8 {
           32 // SPIs start at 32
       }
   }
   ```

---

### Phase 6: Syscalls and Userspace

**Goal:** Run userspace programs

**Tasks:**

1. **Implement `SyscallFrame` trait:**
   ```rust
   impl SyscallFrame for Aarch64ExceptionFrame {
       fn syscall_number(&self) -> u64 { self.x8 }
       fn arg1(&self) -> u64 { self.x0 }
       fn arg2(&self) -> u64 { self.x1 }
       fn arg3(&self) -> u64 { self.x2 }
       fn arg4(&self) -> u64 { self.x3 }
       fn arg5(&self) -> u64 { self.x4 }
       fn arg6(&self) -> u64 { self.x5 }
       fn set_return_value(&mut self, value: u64) { self.x0 = value; }
       fn return_value(&self) -> u64 { self.x0 }
   }
   ```

2. **Handle SVC exception:**
   - Check ESR_EL1 for exception class (EC = 0x15 for SVC)
   - Dispatch to syscall handler

3. **Implement `eret` to userspace:**
   ```rust
   pub unsafe fn return_to_userspace(frame: &Aarch64ExceptionFrame) -> ! {
       // Restore X0-X30
       // Set ELR_EL1 = frame.pc
       // Set SPSR_EL1 = frame.pstate (with EL0 bits)
       // Set SP_EL0 = frame.sp
       asm!("eret");
   }
   ```

4. **Implement `PerCpuOps` trait:**
   ```rust
   impl PerCpuOps for Aarch64PerCpu {
       fn cpu_id() -> u64 {
           let mpidr: u64;
           unsafe { asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
           mpidr & 0xFF // Aff0 = CPU ID within cluster
       }

       fn current_thread_ptr() -> *mut u8 {
           let ptr: u64;
           unsafe { asm!("mrs {}, tpidr_el1", out(reg) ptr) };
           ptr as *mut u8
       }

       unsafe fn set_current_thread_ptr(ptr: *mut u8) {
           asm!("msr tpidr_el1, {}", in(reg) ptr as u64);
       }
       // ... etc
   }
   ```

---

### Phase 7: Feature Parity

**Goal:** ARM64 matches x86-64 functionality

**Tasks:**
1. Migrate all remaining x86-64 specific code
2. Update userspace binaries for ARM64
3. Add ARM64 test programs
4. Full signal support
5. Full filesystem support

---

## Parallels Desktop Setup

### Prerequisites

- Parallels Desktop Pro or Business Edition (CLI access requires Pro+)
- macOS 13+ (Ventura or later)
- Apple Silicon Mac (M1/M2/M3/M4)

### Verify Parallels CLI Access

```bash
# Check prlctl is available
which prlctl
# Should output: /usr/local/bin/prlctl

# Verify version
prlctl --version
# Should show Parallels Desktop 19.x or later
```

### Create ARM64 Linux VM for Kernel Development

We'll use a minimal ARM64 Linux as a base, then replace the kernel with Breenix.

**Option A: Create from ISO (Recommended)**

```bash
# Download Ubuntu Server ARM64
curl -L -o ~/Downloads/ubuntu-24.04-live-server-arm64.iso \
    "https://cdimage.ubuntu.com/releases/24.04/release/ubuntu-24.04-live-server-arm64.iso"

# Create VM
prlctl create "Breenix-ARM64" --ostype linux --distribution ubuntu

# Configure VM
prlctl set "Breenix-ARM64" --cpus 4
prlctl set "Breenix-ARM64" --memsize 4096
prlctl set "Breenix-ARM64" --device-set cdrom0 --image ~/Downloads/ubuntu-24.04-live-server-arm64.iso

# Start VM for installation
prlctl start "Breenix-ARM64"

# After installation, we'll configure for custom kernel boot
```

**Option B: Use Existing Linux and Modify**

```bash
# List existing VMs
prlctl list -a

# Clone an existing ARM64 Linux VM
prlctl clone "Ubuntu-ARM64" --name "Breenix-ARM64"
```

### Configure VM for Custom Kernel Boot

After the base Linux is installed, we need to configure UEFI to boot our kernel.

```bash
# Stop the VM
prlctl stop "Breenix-ARM64"

# Get VM location
prlctl list -i "Breenix-ARM64" | grep "Home"
# Outputs something like: /Users/wrb/Parallels/Breenix-ARM64.pvm

# The EFI variables and boot configuration are inside the .pvm bundle
```

### Kernel Installation Script

Create a script to deploy the Breenix kernel to the VM:

```bash
#!/bin/bash
# scripts/deploy-parallels-arm64.sh

set -e

VM_NAME="Breenix-ARM64"
KERNEL_PATH="target/aarch64-breenix/release/breenix.efi"

# Build kernel
cargo build --target aarch64-breenix.json -Z build-std=core,alloc --release

# Stop VM if running
prlctl stop "$VM_NAME" --kill 2>/dev/null || true

# Mount VM disk and copy kernel
# (This requires the VM to have a shared folder or we use prl_disk_tool)

# Start VM
prlctl start "$VM_NAME"

# Get serial console output
prlctl enter "$VM_NAME" --serial
```

### Alternative: Direct EFI Boot (No Linux Base)

For a pure bare-metal experience without Linux:

1. Create an EFI System Partition image:
   ```bash
   # Create 64MB FAT32 image
   dd if=/dev/zero of=efi_disk.img bs=1M count=64
   mkfs.vfat -F 32 efi_disk.img

   # Mount and copy EFI binary
   mkdir -p /tmp/efi_mount
   hdiutil attach -mountpoint /tmp/efi_mount efi_disk.img
   mkdir -p /tmp/efi_mount/EFI/BOOT
   cp target/aarch64-breenix/release/breenix.efi /tmp/efi_mount/EFI/BOOT/BOOTAA64.EFI
   hdiutil detach /tmp/efi_mount
   ```

2. Create VM with just EFI disk:
   ```bash
   prlctl create "Breenix-Bare" --ostype other --no-hdd
   prlctl set "Breenix-Bare" --device-add hdd --image efi_disk.img
   ```

### Serial Console Access

```bash
# Attach to serial console (COM1)
prlctl enter "Breenix-ARM64" --serial

# Or use screen
screen /dev/ttys000  # Check actual device with `prlctl list -i`
```

### Automated Testing Script

```bash
#!/bin/bash
# scripts/test-parallels-arm64.sh

VM_NAME="Breenix-ARM64"
TIMEOUT=60
LOG_FILE="/tmp/breenix-arm64-serial.log"

# Build
cargo build --target aarch64-breenix.json -Z build-std=core,alloc --release || exit 1

# Deploy kernel (implementation depends on boot method)
./scripts/deploy-parallels-arm64.sh

# Start VM and capture serial output
prlctl start "$VM_NAME"
timeout $TIMEOUT prlctl enter "$VM_NAME" --serial > "$LOG_FILE" 2>&1 &

# Wait for expected output
for i in $(seq 1 $TIMEOUT); do
    if grep -q "KERNEL_POST_TESTS_COMPLETE" "$LOG_FILE" 2>/dev/null; then
        echo "SUCCESS: Kernel boot completed"
        prlctl stop "$VM_NAME" --kill
        exit 0
    fi
    sleep 1
done

echo "TIMEOUT: Kernel did not complete boot"
prlctl stop "$VM_NAME" --kill
cat "$LOG_FILE"
exit 1
```

---

## CI/CD Configuration

### GitHub Actions Workflow

```yaml
# .github/workflows/build.yml
name: Build

on: [push, pull_request]

jobs:
  build-x86_64:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action@stable
        with:
          toolchain: nightly
          components: rust-src, llvm-tools-preview

      - name: Build x86_64
        run: |
          cargo build --target x86_64-breenix.json \
            -Z build-std=core,alloc \
            --release

  build-aarch64:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action@stable
        with:
          toolchain: nightly
          components: rust-src, llvm-tools-preview

      - name: Build aarch64
        run: |
          cargo build --target aarch64-breenix.json \
            -Z build-std=core,alloc \
            --release

  test-x86_64:
    needs: build-x86_64
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install QEMU
        run: sudo apt-get install -y qemu-system-x86

      - name: Run tests
        run: ./docker/qemu/run-boot-parallel.sh 1
```

### Cross-Compilation Notes

- GitHub Actions runners are x86-64
- ARM64 cross-compilation works fine (just can't run tests)
- Consider self-hosted ARM64 runners for full CI testing later
- Alternatively, use QEMU user-mode emulation for simple tests

---

## Testing Strategy

### Development Workflow

1. **Code changes** - Edit in `breenix-arm64` worktree
2. **Build check** - `cargo build --target aarch64-breenix.json`
3. **Local test** - Run in Parallels
4. **Cross-arch verify** - `cargo build --target x86_64-breenix.json`
5. **PR** - CI builds both, tests x86-64

### QEMU Testing (Before Parallels Works)

For early development, use QEMU for ARM64 testing:

```bash
# Run ARM64 kernel in QEMU
qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 1024 \
    -nographic \
    -kernel target/aarch64-breenix/release/breenix.elf \
    -serial mon:stdio
```

This is useful for debugging before Parallels is set up.

---

## Codex Integration

When using Codex to implement phases:

1. **Always specify the target file path** explicitly
2. **Include the trait definition** from `traits.rs` in context
3. **Reference the x86-64 implementation** as a pattern
4. **Verify with `cargo build --target aarch64-breenix.json`** after each change

### Example Codex Prompt

```
Implement the CpuOps trait for ARM64 in kernel/src/arch_impl/aarch64/cpu.rs.

Reference the trait definition:
[paste from traits.rs]

Reference the x86_64 implementation:
[paste from x86_64/cpu.rs]

ARM64 specifics:
- Use `daifset`/`daifclr` for interrupt enable/disable
- The I bit (bit 7) in DAIF controls IRQs
- Use `wfi` for halt
- Use `mrs`/`msr` for system register access
```

---

## Resources

### ARM Architecture References

- [ARM Architecture Reference Manual (ARMv8-A)](https://developer.arm.com/documentation/ddi0487/latest)
- [ARM Cortex-A Programmer's Guide](https://developer.arm.com/documentation/den0024/latest)
- [ARM Exception Handling](https://developer.arm.com/documentation/100933/latest)

### Rust on ARM64

- [aarch64-cpu crate](https://docs.rs/aarch64-cpu)
- [tock-registers crate](https://docs.rs/tock-registers)
- [Rust Embedded Book](https://docs.rust-embedded.org/book/)

### QEMU ARM64

- [QEMU ARM System Emulator](https://www.qemu.org/docs/master/system/arm/virt.html)
- [QEMU virt machine](https://www.qemu.org/docs/master/system/arm/virt.html)

### Parallels

- [Parallels CLI Reference](https://download.parallels.com/desktop/v18/docs/en_US/Parallels%20Desktop%20Command-Line%20Reference.pdf)
- [Parallels KB: Apple Silicon](https://kb.parallels.com/125343)

---

## Milestones Checklist

- [ ] **Phase 1**: Both architectures compile
  - [ ] `aarch64-breenix.json` created
  - [ ] `arch_impl/aarch64/` module structure
  - [ ] Stub trait implementations
  - [ ] CI builds both targets

- [ ] **Phase 2**: Serial output works
  - [ ] PL011 UART driver
  - [ ] "Hello from ARM64!" prints
  - [ ] `CpuOps` trait implemented

- [ ] **Phase 3**: Exception handling works
  - [ ] Exception vector table
  - [ ] Synchronous exception handler
  - [ ] IRQ handler
  - [ ] `InterruptFrame` trait implemented

- [ ] **Phase 4**: Memory management works
  - [ ] 4-level page tables
  - [ ] MMU enabled
  - [ ] `PageTableOps` and `PageFlags` traits implemented

- [ ] **Phase 5**: Timer interrupts work
  - [ ] Generic timer configured
  - [ ] GICv2 driver
  - [ ] `TimerOps` and `InterruptController` traits implemented

- [ ] **Phase 6**: Userspace works
  - [ ] Syscall handling (SVC)
  - [ ] EL0 transition
  - [ ] `SyscallFrame` and `PerCpuOps` traits implemented

- [ ] **Phase 7**: Feature parity
  - [ ] All x86-64 tests pass on ARM64
  - [ ] Signal handling
  - [ ] Filesystem

---

## Contact

- **Primary Branch Maintainer**: Claude + Codex collaboration
- **x86-64 Reference**: See `feature/signal-completion` branch
- **Questions**: Create issues or discuss in PR reviews
