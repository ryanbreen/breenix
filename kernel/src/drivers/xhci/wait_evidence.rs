//! Bounded, always-on command/wake history. Only a failed replay prints it.
//! The worker alone writes command slots. IRQ and softirq use separate atomics;
//! readers skip a slot being overwritten rather than waiting for its writer.
use core::sync::atomic::{AtomicU64, Ordering};
const SLOTS: usize = 8;
struct Command {
    seq: AtomicU64,
    fields: [AtomicU64; 8],
}
impl Command {
    const fn new() -> Self {
        Self { seq: AtomicU64::new(0), fields: [const { AtomicU64::new(0) }; 8] }
    }
}
static COMMANDS: [Command; SLOTS] = [const { Command::new() }; SLOTS];
static SEQ: AtomicU64 = AtomicU64::new(0);
static IRQ: AtomicU64 = AtomicU64::new(0);
static IRQ_WAITER: AtomicU64 = AtomicU64::new(0);
static IRQ_CPU: AtomicU64 = AtomicU64::new(0);
static IRQ_PREEMPT: AtomicU64 = AtomicU64::new(0);
static WAKES: AtomicU64 = AtomicU64::new(0);
static PHASE: AtomicU64 = AtomicU64::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static WORKER_CPU: AtomicU64 = AtomicU64::new(0);
static WORKER_PREEMPT: AtomicU64 = AtomicU64::new(0);
static OUTPUT: [crate::log_notice::Record; SLOTS + 1] =
    [const { crate::log_notice::Record::new() }; SLOTS + 1];
pub(super) fn irq(waiter: u64) {
    IRQ_WAITER.store(waiter, Ordering::Relaxed);
    IRQ_CPU.store(crate::task::scheduler::current_cpu() as u64, Ordering::Relaxed);
    IRQ_PREEMPT.store(crate::per_cpu::preempt_count() as u64, Ordering::Relaxed);
    IRQ.fetch_add(1, Ordering::Release);
}
pub(super) fn wake() { WAKES.fetch_add(1, Ordering::Relaxed); }
pub(super) fn begin(wanted: u64, kind: u32, command: u32, deadline: u64, irq: u64) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let slot = &COMMANDS[(seq as usize - 1) % SLOTS];
    slot.seq.store(0, Ordering::SeqCst);
    for (field, value) in slot.fields.iter().zip([wanted, kind as u64, command as u64, super::now(), deadline, irq, 0, 0]) {
        field.store(value, Ordering::SeqCst);
    }
    slot.seq.store(seq, Ordering::SeqCst);
    PHASE.store(1, Ordering::Relaxed);
}
pub(super) fn blocked(deadline: u64) {
    DEADLINE.store(deadline, Ordering::Relaxed);
    WORKER_CPU.store(crate::task::scheduler::current_cpu() as u64, Ordering::Relaxed);
    WORKER_PREEMPT.store(crate::per_cpu::preempt_count() as u64, Ordering::Relaxed);
    PHASE.store(2, Ordering::Release);
}
pub(super) fn returned(work: bool) { PHASE.store(if work { 3 } else { 4 }, Ordering::Release); }
pub(super) fn end(code: u8) {
    let seq = SEQ.load(Ordering::Relaxed);
    let slot = &COMMANDS[(seq as usize - 1) % SLOTS];
    slot.seq.store(0, Ordering::SeqCst);
    slot.fields[6].store(super::now(), Ordering::SeqCst);
    slot.fields[7].store(code as u64, Ordering::SeqCst);
    slot.seq.store(seq, Ordering::SeqCst);
}
pub(super) fn report() {
    let irq = IRQ.load(Ordering::Acquire);
    let waiter = IRQ_WAITER.load(Ordering::Relaxed);
    let cpu = IRQ_CPU.load(Ordering::Relaxed);
    let preempt = IRQ_PREEMPT.load(Ordering::Relaxed);
    let wakes = WAKES.load(Ordering::Relaxed);
    let phase = PHASE.load(Ordering::Acquire);
    let deadline = DEADLINE.load(Ordering::Relaxed);
    let worker_cpu = WORKER_CPU.load(Ordering::Relaxed);
    let worker_preempt = WORKER_PREEMPT.load(Ordering::Relaxed);
    OUTPUT[0].post_again(format_args!("XHCI_REPLAY_WAIT irq={irq} irq_waiter={waiter} irq_cpu={cpu} irq_preempt={preempt} wakes={wakes} phase={phase} deadline={deadline} worker_cpu={worker_cpu} worker_preempt={worker_preempt}"));
    for (slot, output) in COMMANDS.iter().zip(&OUTPUT[1..]) {
        let seq = slot.seq.load(Ordering::SeqCst);
        let [wanted, kind, command, began, deadline, irq, end, code] =
            core::array::from_fn(|i| slot.fields[i].load(Ordering::SeqCst));
        if seq != 0 && seq == slot.seq.load(Ordering::SeqCst) {
            output.post_again(format_args!("XHCI_REPLAY_COMMAND seq={seq} wanted={wanted:#x} kind={kind} command={command:#x} began={began} deadline={deadline} irq={irq} end={end} code={code}"));
        }
    }
}
