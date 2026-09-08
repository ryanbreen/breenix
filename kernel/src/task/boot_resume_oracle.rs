//! Pre-userspace x86 register witnesses for 567.

use core::sync::atomic::{AtomicU64, Ordering};

use super::{kthread, scheduler};

const CYCLES: u64 = 32;
static REQUEST: AtomicU64 = AtomicU64::new(0);
static ACK: AtomicU64 = AtomicU64::new(0);

/// Caller has IF=0. STI's shadow covers HLT; the instruction at label 2
/// captures the resumed RFLAGS before CLI or comparisons change them.
/// R10 witnesses RSP and R11 witnesses the resume label. The remaining
/// checked registers carry distinct constants. CLD precedes the Rust return.
#[unsafe(naked)]
unsafe extern "C" fn witness_resume() -> u64 {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov r10, rsp",
        "lea r11, [rip + 2f]",
        "mov rbx, 0x567b",
        "mov rbp, 0x567d",
        "mov r12, 0x5612",
        "mov r13, 0x5613",
        "mov r14, 0x5614",
        "mov r15, 0x5615",
        "mov rcx, 0x567c",
        "mov rdx, 0x567e",
        "mov rsi, 0x5675",
        "mov rdi, 0x5676",
        "mov r8, 0x5678",
        "mov r9, 0x5679",
        "mov rax, 0x567a",
        "push 0x47",
        "popfq",
        "sti",
        "hlt",
        "2:",
        "pushfq",
        "cli",
        "cld",
        "cmp qword ptr [rsp], 0x247",
        "jne 3f",
        "cmp rax, 0x567a",
        "jne 3f",
        "lea rax, [rsp + 8]",
        "cmp rax, r10",
        "jne 3f",
        "lea rax, [rip + 2b]",
        "cmp rax, r11",
        "jne 3f",
        "cmp rbx, 0x567b",
        "jne 3f",
        "cmp rbp, 0x567d",
        "jne 3f",
        "cmp r12, 0x5612",
        "jne 3f",
        "cmp r13, 0x5613",
        "jne 3f",
        "cmp r14, 0x5614",
        "jne 3f",
        "cmp r15, 0x5615",
        "jne 3f",
        "cmp rcx, 0x567c",
        "jne 3f",
        "cmp rdx, 0x567e",
        "jne 3f",
        "cmp rsi, 0x5675",
        "jne 3f",
        "cmp rdi, 0x5676",
        "jne 3f",
        "cmp r8, 0x5678",
        "jne 3f",
        "cmp r9, 0x5679",
        "jne 3f",
        "xor eax, eax",
        "jmp 4f",
        "3:",
        "mov eax, 1",
        "4:",
        "add rsp, 8",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    );
}

pub fn run() {
    let enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let peer = kthread::kthread_run(
        || {
            for cycle in 1..=CYCLES {
                while REQUEST.load(Ordering::Acquire) < cycle {
                    kthread::kthread_park_if(|| REQUEST.load(Ordering::Acquire) < cycle);
                }
                ACK.store(cycle, Ordering::Release);
            }
        },
        "boot-resume-witness",
    )
    .expect("boot resume oracle peer");
    let deadline = crate::time::get_monotonic_time().saturating_add(30_000);
    let mut cycles = 0;
    let mut mismatch = 0;
    for cycle in 1..=CYCLES {
        REQUEST.store(cycle, Ordering::Release);
        kthread::kthread_unpark(&peer);
        loop {
            scheduler::yield_current();
            // Exercise the public x86 entry with IF=0 before the witness.
            // It must request dispatch without changing the snapshot owner.
            scheduler::schedule();
            // IF=0 before entry and after return. No Rust runs with DF=1.
            mismatch += unsafe { witness_resume() };
            if ACK.load(Ordering::Acquire) == cycle {
                cycles += 1;
                break;
            }
            if crate::time::get_monotonic_time() >= deadline {
                break;
            }
        }
        if cycles != cycle {
            break;
        }
    }
    // Printing is ordinary boot-thread context, outside the interrupt return.
    if enabled {
        x86_64::instructions::interrupts::enable();
    }
    crate::serial_println!(
        "[BOOT_RESUME_ORACLE:x86:cycles={}:mismatch={}:{}]",
        cycles,
        mismatch,
        if cycles == CYCLES && mismatch == 0 {
            "PASS"
        } else {
            "FAIL"
        },
    );
}
