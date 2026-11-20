//! Suite failure snapshots for x86. NMI delivery can sample a CPU with IF clear.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use super::scheduler::{self, MAX_CPUS};
use crate::arch_impl::x86_64::smp;
use crate::log_notice::Record;
use crate::syscall::{errno::{EINVAL, ENOSYS, EPERM}, SyscallResult};

#[path = "cpu_sample.rs"]
mod cpu_sample;
#[path = "cpu_progress/record.rs"]
mod record;
use cpu_sample::Sample;
use record::{Capture, CpuSample, Reason, NAME_BYTES};

pub(crate) const PR_CPU_PROGRESS: u64 = 0x4252_0009;
const REPORT: u64 = 0;
const TIMEOUT: u64 = 1;
const PIN: u64 = 2;

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static ACTIVE: Mutex<Option<Capture>> = Mutex::new(None);
static REQUESTS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SAMPLES: [Sample<11>; MAX_CPUS] = [const { Sample::new() }; MAX_CPUS];
static NOTICES: [Record; MAX_CPUS] = [const { Record::new() }; MAX_CPUS];

pub(crate) fn sys_cpu_progress(action: u64) -> SyscallResult {
    if !crate::test_control::enabled() {
        return SyscallResult::Err(ENOSYS as u64);
    }
    let root = super::thread::current_thread_signals()
        .and_then(|signals| signals.mirror.identity())
        .is_some_and(|identity| identity.euid == 0);
    if !root {
        return SyscallResult::Err(EPERM as u64);
    }
    match action {
        REPORT => report(),
        TIMEOUT => request(Reason::Timeout),
        PIN => request(Reason::Pin),
        _ => return SyscallResult::Err(EINVAL as u64),
    }
    SyscallResult::Ok(0)
}

fn request(reason: Reason) {
    crate::arch_without_interrupts(|| {
        let Some(mut active) = ACTIVE.try_lock() else { return };
        if active.is_some() {
            return;
        }
        let me = crate::per_cpu::cpu_id();
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let reporter = crate::per_cpu::current_thread_id_lock_free().unwrap_or(0);
        *active = Some(Capture { sequence, reporter, reason, tick: crate::time::get_ticks() });
        // A late NMI for a previous request must not nest this CPU's writer.
        REQUESTS[me].store(0, Ordering::Release);
        store(me, sequence, 0, false, true);
        for cpu in (0..MAX_CPUS).filter(|&cpu| cpu != me && smp::is_cpu_online(cpu)) {
            // Keep an unacknowledged request's sequence so a late reply
            // still belongs to the capture that requested it.
            crate::memory::tlb::request_cpu_progress(cpu, || {
                REQUESTS[cpu].compare_exchange(0, sequence, Ordering::Release, Ordering::Relaxed).is_ok()
            });
        }
    });
}

pub(crate) fn record_nmi(cpu: usize, frame: &InterruptStackFrame) {
    let sequence = REQUESTS[cpu].swap(0, Ordering::AcqRel);
    if sequence != 0 {
        use x86_64::registers::rflags::RFlags;
        store(cpu, sequence, frame.instruction_pointer.as_u64(),
            frame.cpu_flags.contains(RFlags::INTERRUPT_FLAG), false);
    }
}

fn store(cpu: usize, sequence: u64, pc: u64, interrupts: bool, caller: bool) {
    let data = crate::per_cpu::cpu_data(cpu);
    let mut name = [0u8; NAME_BYTES];
    // SAFETY: this is the locally interrupted CPU (found by APIC id in NMI
    // context), or the IRQ-masked reporting CPU. The current thread cannot
    // leave this CPU underneath the read. Use its local slot without GS, which
    // may still contain a userspace base inside an entry/return stub.
    let (tid, preempt) = unsafe {
        let current = (&raw const (*data).current_thread).read_volatile();
        let tid = if current.is_null() {
            0
        } else {
            // Name and ID are immutable after scheduler publication.
            let bytes = (&*(&raw const (*current).name)).as_bytes();
            let len = bytes.len().min(name.len());
            name[..len].copy_from_slice(&bytes[..len]);
            (&raw const (*current).id).read_volatile()
        };
        (tid, (&raw const (*data).preempt_count).read_volatile())
    };
    let (schedule_tick, queue_len, pinned_holds) = scheduler::cpu_progress_queue(cpu);
    let flags = u64::from(preempt) | (u64::from(interrupts) << 32) | (u64::from(caller) << 33);
    SAMPLES[cpu].store([
        sequence, crate::time::get_ticks(), tid, flags, schedule_tick, u64::from(queue_len),
        pinned_holds, pc,
        u64::from_ne_bytes(name[0..8].try_into().unwrap()),
        u64::from_ne_bytes(name[8..16].try_into().unwrap()),
        u64::from_ne_bytes(name[16..24].try_into().unwrap()),
    ]);
}

fn report() {
    let Some(mut active) = ACTIVE.try_lock() else { return };
    let Some(capture) = active.take() else { return };
    for cpu in (0..MAX_CPUS).filter(|&cpu| smp::is_cpu_online(cpu)) {
        let sample = SAMPLES[cpu].load().map(|[sequence, tick, tid, flags, schedule_tick, queue_len, pinned_holds, pc, a, b, c]| {
            let mut name = [0; NAME_BYTES];
            for (bytes, word) in name.chunks_exact_mut(8).zip([a, b, c]) {
                bytes.copy_from_slice(&word.to_ne_bytes());
            }
            CpuSample { sequence, tick, tid, name, preempt: flags as u32,
                interrupts: flags & (1 << 32) != 0, caller: flags & (1 << 33) != 0,
                schedule_tick, queue_len: queue_len as u32, pinned_holds, pc }
        });
        let line = record::line(capture, cpu, sample);
        NOTICES[cpu].post_again(format_args!("{}", line.as_str().trim_matches('\n')));
    }
}
