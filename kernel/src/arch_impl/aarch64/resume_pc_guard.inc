// Shared resume-PC admission and refusal-record helpers. Feature-dependent
// codegen can place multiple global_asm! blocks in one assembler unit, so the
// symbol guard keeps repeated include_str! expansion single-defined.
.ifndef BREENIX_RESUME_PC_GUARD_INC
.set BREENIX_RESUME_PC_GUARD_INC, 1

// \pc is left untouched. \s0 \s1 \s2 are scratch registers the caller
// guarantees are dead. Branches to \ok when \pc names a 4-byte-aligned
// address inside kernel text in any of the three mappings the Rust predicate
// admits; falls through otherwise.
.macro RESUME_PC_EL1_OK pc, s0, s1, s2, ok
    tst   \pc, #3
    b.ne  8888f
    adrp  \s0, __kernel_text_start
    add   \s0, \s0, :lo12:__kernel_text_start
    adrp  \s1, __kernel_text_end
    add   \s1, \s1, :lo12:__kernel_text_end
    sub   \s1, \s1, \s0              // s1 = text size
    sub   \s2, \pc, \s0
    cmp   \s2, \s1
    b.lo  \ok
    mov   \s2, #KERNEL_VIRT_BASE
    add   \s2, \pc, \s2              // higher alias
    sub   \s2, \s2, \s0
    cmp   \s2, \s1
    b.lo  \ok
    mov   \s2, #KERNEL_VIRT_BASE
    sub   \s2, \pc, \s2              // lower alias
    sub   \s2, \s2, \s0
    cmp   \s2, \s1
    b.lo  \ok
8888:
.endm

// Do not exclude the kernel-text window here. On Parallels the kernel is
// identity-mapped around 0x4008_0000, overlapping the 0x4000_0000 and
// 0x4100_0000 ranges where userspace programs are linked.
//
// Branches to \ok when \pc is a plausible EL0 resume PC: 4-byte aligned, at
// or above 0x1000, and not a high-half kernel VA. Falls through otherwise.
.macro RESUME_PC_EL0_OK pc, s0, ok
    tst   \pc, #3
    b.ne  8889f
    cmp   \pc, #1, lsl #12
    b.lo  8889f
    mov   \s0, #KERNEL_VIRT_BASE
    cmp   \pc, \s0
    b.lo  \ok
8889:
.endm

// \percpu already holds tpidr_el1 and is known non-zero. \src is an
// immediate. \s0 \s1 are dead scratch. \fsp holds the exception-frame base.
.macro RESUME_PC_RECORD percpu, fsp, pc, spsr, src, s0, s1
    str   \pc,   [\percpu, #PERCPU_ERET_GUARD_ELR]
    str   \spsr, [\percpu, #PERCPU_ERET_GUARD_SPSR]
    ldr   \s0, [\fsp, #232]                      // frame.x29
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_X29]
    ldr   \s0, [\fsp, #240]                      // frame.x30
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_X30]
    add   \s0, \fsp, #272                        // interrupted SP
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_SP]
    ldr   \s0, [\percpu, #PERCPU_ERET_GUARD_COUNT]
    add   \s0, \s0, #1
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_COUNT]
    ldr   \s0, [\percpu, #PERCPU_IDLE_STACK_TOP]
    cbz   \s0, 8890f
    // The refused thread's stack may be reclaimed after the record drains.
    // Point both later SP-selection paths at this CPU's own idle stack before
    // publishing the refusal and redirecting execution to idle.
    str   \s0, [\percpu, #16]                    // kernel_stack_top = idle stack
    str   \s0, [\percpu, #40]                    // user_sp_scratch = idle stack
    str   xzr, [\percpu, #48]                    // drop nested EL1 resume SP
8890:
    mov   \s1, #\src
    str   \s1, [\percpu, #PERCPU_ERET_GUARD_SOURCE]
.endm

// Record a refusal from a ret-dispatch site that has no exception frame.
// \percpu is known non-zero; \s0 and \s1 are dead scratch registers.
//
// The frame-based arms leave the refused thread's kernel stack in the ERET
// epilogue, which re-selects SP from percpu+16/+40 before returning. This arm
// has no epilogue: the caller has already executed `mov sp, <ctx.sp>` and
// branches straight to idle on that SP. So the pivot has to happen here, and
// the ORDER IS LOAD-BEARING: SP moves to the CPU-owned idle stack BEFORE
// +16/+40 stop naming the refused thread's stack. Those two words are the
// entire aarch64 is_kernel_stack_slot_live() predicate, and idle's first act
// is run_deferred_reclamation() -> drain_asm_resume_pc_refusals() ->
// set_terminated(tid) -> reclaim_terminated_threads(). Repointing them while
// this CPU still executes on that stack would hand a running stack to the
// reaper (free + 64 KiB scrub). Pivoting first leaves no such instant: before
// the pivot the words name the stack the CPU is on; between pivot and repoint
// they over-name a stack nobody is on (safe); after, they name the idle stack
// the CPU is actually running on. When idle_stack_top is not published yet
// there is nowhere to pivot to, so neither the pivot nor the repoint happens
// and custody keeps naming the stack in use.
.macro RESUME_PC_RECORD_NOFRAME percpu, pc, src, s0, s1
    str   \pc, [\percpu, #PERCPU_ERET_GUARD_ELR]
    str   xzr, [\percpu, #PERCPU_ERET_GUARD_SPSR]
    str   xzr, [\percpu, #PERCPU_ERET_GUARD_X29]
    str   xzr, [\percpu, #PERCPU_ERET_GUARD_X30]
    mov   \s0, sp
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_SP]  // pre-pivot SP, for forensics
    ldr   \s0, [\percpu, #PERCPU_ERET_GUARD_COUNT]
    add   \s0, \s0, #1
    str   \s0, [\percpu, #PERCPU_ERET_GUARD_COUNT]
    ldr   \s0, [\percpu, #PERCPU_IDLE_STACK_TOP]
    cbz   \s0, 8891f
    // Leave the refused thread's stack for real, then stop naming it.
    mov   sp, \s0
    str   \s0, [\percpu, #16]
    str   \s0, [\percpu, #40]
    str   xzr, [\percpu, #48]
8891:
    mov   \s1, #\src
    str   \s1, [\percpu, #PERCPU_ERET_GUARD_SOURCE]
.endm

.endif
