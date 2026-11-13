//! One suite request at a time; only the owning xHCI worker touches its controller.
use crate::{input::control::Receipt, syscall::errno::*};
use crate::task::completion::Completion;
use core::sync::atomic::{AtomicU8, Ordering};
#[derive(Clone, Copy)]
pub(super) enum Request { Keyboard, Pointer(i32, i32) }
const IDLE: u8 = 0;
const POSTED: u8 = 1;
const TAKEN: u8 = 2;
const ANSWERED: u8 = 3;
const ABANDONED: u8 = 4;
static STATE: AtomicU8 = AtomicU8::new(IDLE);
static PENDING: spin::Mutex<Option<(Request, bool)>> = spin::Mutex::new(None);
static ANSWER: spin::Mutex<Option<Result<Receipt, u64>>> = spin::Mutex::new(None);
static ANSWER_READY: Completion = Completion::new();
static DIAGNOSTICS: [crate::log_notice::Record; 4] =
    [const { crate::log_notice::Record::new() }; 4];

fn report_killed(state: &str, posted_at: u64) {
    super::wait_evidence::report();
    super::report_wait_hardware();
    let age_ns = super::now().saturating_sub(posted_at);
    let worker_tid = super::WORKER_TID.load(Ordering::Acquire);
    let snapshot = crate::task::scheduler::try_thread_wait(worker_tid);
    let worker_state = snapshot.map(|(state, _, _)| state);
    let cpu = snapshot.and_then(|(_, cpu, _)| cpu);
    let wake_time_ns = snapshot.and_then(|(_, _, deadline)| deadline);
    let work_pending = super::WORK.pending();
    let waiter = super::WORK.waiter();
    let leds = super::LEDS.load(Ordering::Acquire);
    let interrupts = super::interrupt_count();
    // Deferred to the existing thread-context notice drain. A busy record is
    // tried once; no serial lock, UART wait, allocation or mailbox lock here.
    for record in &DIAGNOSTICS {
        if record.post_again(format_args!("XHCI_REPLAY_STATE state={state} age_ns={age_ns} worker_tid={worker_tid} worker_state={worker_state:?} cpu={cpu:?} wake_time_ns={wake_time_ns:?} work_pending={work_pending} waiter={waiter} leds={leds} interrupts={interrupts}")) {
            break;
        }
    }
}

pub(super) fn pending() -> bool { STATE.load(Ordering::Acquire) == POSTED }
pub(super) fn take() -> Option<(Request, bool)> {
    if STATE.compare_exchange(POSTED, TAKEN, Ordering::AcqRel, Ordering::Acquire).is_err() { return None; }
    crate::arch_without_interrupts(|| PENDING.lock().take())
}
pub(super) fn answer(result: Result<Receipt, u64>) {
    // Keep the answer lock through completion so a new submit cannot reset the
    // completion before this worker has finished signalling the old one. The
    // worker must not be preempted with a mailbox lock held: its woken caller
    // can otherwise spin on that lock on the same CPU.
    crate::arch_without_interrupts(|| {
        let mut answer = ANSWER.lock();
        *answer = Some(result);
        if STATE.compare_exchange(TAKEN, ANSWERED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            ANSWER_READY.complete(1);
        } else {
            answer.take();
            let _ = STATE.compare_exchange(ABANDONED, IDLE, Ordering::AcqRel, Ordering::Acquire);
        }
    });
}
pub(super) fn submit(request: Request, suppress: bool) -> Result<Receipt, u64> {
    if !crate::test_control::enabled() { return Err(ENOSYS as u64); }
    if super::FAILED.load(Ordering::Acquire) { return Err(EIO as u64); }
    // TAKEN reserves the mailbox while the request and completion are prepared.
    if STATE.compare_exchange(IDLE, TAKEN, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(EBUSY as u64);
    }
    ANSWER_READY.reset();
    *ANSWER.lock() = None;
    *PENDING.lock() = Some((request, suppress));
    let posted_at = super::now();
    let _ = STATE.compare_exchange(TAKEN, POSTED, Ordering::Release, Ordering::Relaxed);
    // request() supplies the existing publish/check/wake ordering.
    super::request(&super::LEDS);
    loop {
        let wait = ANSWER_READY.wait_timeout_killable(1, 1_000_000_000);
        if wait == Ok(true) || wait == Err(EINTR) {
            // Serialize cleanup with the worker's completion signal and with
            // withdrawal of a request that the worker has not yet taken.
            let mut pending = PENDING.lock();
            let mut answer = ANSWER.lock();
            loop {
                let state = STATE.load(Ordering::Acquire);
                match state {
                    ANSWERED => {
                        let result = answer.take().expect("answered replay has a receipt");
                        let _ = STATE.compare_exchange(ANSWERED, IDLE, Ordering::AcqRel, Ordering::Acquire);
                        return result;
                    }
                    POSTED if wait == Err(EINTR) => {
                        if STATE.compare_exchange(POSTED, IDLE, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                            pending.take();
                            drop(answer);
                            drop(pending);
                            report_killed("Posted", posted_at);
                            return Err(EINTR as u64);
                        }
                    }
                    TAKEN if wait == Err(EINTR) => {
                        if STATE.compare_exchange(TAKEN, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                            drop(answer);
                            drop(pending);
                            report_killed("Taken", posted_at);
                            return Err(EINTR as u64);
                        }
                    }
                    _ => break,
                }
            }
        }
        if super::FAILED.load(Ordering::Acquire) {
            PENDING.lock().take();
            STATE.store(IDLE, Ordering::Release);
            return Err(EIO as u64);
        }
    }
}
