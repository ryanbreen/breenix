# Breenix OS - Gemini Agent Guidelines

## Project Overview

Breenix is a production-quality x86_64 operating system kernel written in Rust. This is not a toy or learning project - we follow Linux/FreeBSD standard practices and prioritize quality over speed.

## Core Mandates & Philosophy

*   **Quality First**: Zero tolerance for warnings (compiler or clippy). Every build must be clean.
*   **Testing Integrity**: Never fake a passing test. If a test fails, fix the code or explicitly disable the test with a reason.
*   **GDB for Debugging**: **Do NOT** add logging to hot paths (syscalls, interrupts) for debugging. Use GDB.
*   **Atomic Migrations**: Legacy code removal must happen in the *same commit* as the new feature completion.
*   **Pristine Interrupt Paths**: Interrupt and syscall paths must remain minimal (<1000 cycles). No logging, no allocs, no locks in these paths.

## Project Structure

*   `kernel/`: Core kernel (no_std, no_main)
*   `src.legacy/`: Previous implementation (being phased out)
*   `libs/`: libbreenix, tiered_allocator
*   `tests/`: Integration tests
*   `docs/planning/`: Phase directories (00-15)
*   `xtask/`: Build and test automation tool

## Primary Workflows

### 1. Standard Development & Verification (Boot Stages)
For normal development, use the boot stages test to verify kernel health:

```bash
# Run boot stages test - verifies kernel progresses through all checkpoints
cargo run -p xtask -- boot-stages

# Build only (no execution)
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

### 2. Debugging (Mandatory GDB Usage)
Use GDB when you need to understand *why* something is failing.

**Interactive GDB Session (Primary):**
```bash
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::kernel_main"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
# ... examine state ...
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

**Quick Debug Loop (for signal detection):**
```bash
breenix-kernel-debug-loop/scripts/quick_debug.py --signal "KERNEL_INITIALIZED" --timeout 15
```

### 3. CI Failure Analysis
When a CI run fails or a local test times out/crashes:
```bash
breenix-ci-failure-analysis/scripts/analyze_ci_failure.py target/xtask_output.txt
```

### 4. Code Quality Check (Pre-Commit)
Before every commit, verify zero warnings:
```bash
cd kernel
cargo clippy --target x86_64-unknown-none
cargo build --target x86_64-unknown-none 2>&1 | grep warning
# Must produce NO output
```

## Skills Reference

I am equipped with specific skills for Breenix development. I will invoke these strategies when appropriate.

### Debugging & Analysis
*   **`breenix-gdb-chat`**: Conversational GDB. Use for interactive debugging, crashes, page faults.
*   **`breenix-kernel-debug-loop`**: Fast iterative debugging. Use for checking if specific log signals appear within a timeout.
*   **`breenix-log-analysis`**: Search and analyze kernel logs. Use `scripts/find-in-logs`.
*   **`breenix-systematic-debugging`**: Document-driven debugging (Problem -> Root Cause -> Solution -> Evidence).
*   **`breenix-memory-debugging`**: diagnosing page faults, double faults, and allocator issues.
*   **`breenix-boot-analysis`**: Analyzing boot sequence, timing, and initialization order.
*   **`breenix-interrupt-trace`**: QEMU-based interrupt tracing for low-level analysis.
*   **`breenix-register-watch`**: Debugging register corruption using GDB snapshots.

### Development & Maintenance
*   **`breenix-code-quality-check`**: Enforcing zero-warning policy and Breenix coding standards.
*   **`breenix-legacy-migration`**: Systematic migration from `src.legacy/` to `kernel/`.
*   **`breenix-integration-test-authoring`**: Creating shared QEMU tests (`tests/shared_qemu.rs`).
*   **`breenix-github-workflow-authoring`**: Creating/updating CI workflows.
*   **`breenix-interrupt-syscall-development`**: Guidelines for writing pristine interrupt/syscall code.

## Critical Technical Guidelines

### 🚨 Prohibited Code Sections 🚨
Do NOT modify these without explicit user approval and justification (GDB is usually the answer):
*   **Tier 1 (Forbidden):** `kernel/src/syscall/handler.rs`, `kernel/src/syscall/time.rs`, `kernel/src/syscall/entry.asm`, `kernel/src/interrupts/timer.rs`, `kernel/src/interrupts/timer_entry.asm`.
*   **Tier 2 (High Scrutiny):** `kernel/src/interrupts/context_switch.rs`, `kernel/src/interrupts/mod.rs`, `kernel/src/gdt.rs`, `kernel/src/per_cpu.rs`.

### Interrupt/Syscall Path Rules
*   **NO** serial output or logging.
*   **NO** memory allocation (heap).
*   **NO** page table walks.
*   **NO** locks (unless try_lock with fallback).
*   Target: <1000 cycles total.

### Legacy Migration
1.  Implement feature in `kernel/`.
2.  Verify parity with `src.legacy/` via tests.
3.  Remove `src.legacy/` code AND update `FEATURE_COMPARISON.md` in the **SAME** commit.

### Testing
*   Most tests use `tests/shared_qemu.rs`.
*   Tests wait for signals like `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`.
*   Userspace tests are in `userspace/programs/`.

## Userland Development Stages (Reference)
*   **Stage 1**: libbreenix (Rust) - ~80% Complete (syscall wrappers)
*   **Stage 2**: Rust Runtime - Planned (allocator, panic handler)
*   **Stage 3**: C libc Port - Planned
*   **Stage 4**: Shell - Planned
*   **Stage 5**: Coreutils - Planned

## Logging & Artifacts
*   Logs: `logs/breenix_YYYYMMDD_HHMMSS.log`
*   View latest: `ls -t logs/*.log | head -1 | xargs less`
