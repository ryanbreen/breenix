//! E2.2's host checks (design §8, "E2.2: the park contract", Tests).
//!
//! These guard a mechanism this PR adds, so they cannot be red on the tree
//! before it: `park_core` does not exist there. The census that is red on the
//! old tree is `tests/park_contract_structure.rs`.
//!
//! E2.3's checks (design §8, "E2.3: parking and parked") are at the end: the
//! interrupt exit's decision, `exit_switch`, which both exits call.

use crate::budget::with_budget;
use std::cell::Cell;
use crate::event_queue::EventQueue;
use crate::model::{Action, At, Event, Model, State, EINTR, TID};
use crate::park_core::{
    classify, exit_switch, wait, wait_any, Arrival, AtomicReason, Braked, ContextInputs, ExitSwitch, ExitThread,
    ParkContext, ParkError, Phase, Point, Receipt, ReceiptRing, WaitEnd, WaitMode, WaitWord,
};

const SITE: u64 = 0x5173;

fn sleepable() -> ContextInputs {
    ContextInputs { dispatching: true, has_thread: true, ..ContextInputs::default() }
}

// ----------------------------------------------------------- the decision

#[test]
fn context_table_names_each_context_the_design_lists() {
    use AtomicReason::*;
    let rows: [(&str, ContextInputs, ParkContext); 14] = [
        ("kernel thread, preemptible", sleepable(), ParkContext::Sleepable),
        ("ARM64 braked syscall", ContextInputs { depth: 1, baseline: 1, ..sleepable() }, ParkContext::Sleepable),
        (
            "x86 braked syscall, IF clear",
            ContextInputs { depth: 1, baseline: 1, irqs_masked: true, baseline_irqs_masked: true, ..sleepable() },
            ParkContext::Sleepable,
        ),
        ("kernel thread under park::Braked", ContextInputs { depth: 1, baseline: 1, ..sleepable() }, ParkContext::Sleepable),
        ("kernel thread holding a level", ContextInputs { depth: 1, ..sleepable() }, ParkContext::Atomic(LocksHeld(1))),
        ("syscall holding a level", ContextInputs { depth: 2, baseline: 1, ..sleepable() }, ParkContext::Atomic(LocksHeld(1))),
        ("depth below the baseline", ContextInputs { depth: 0, baseline: 1, ..sleepable() }, ParkContext::Atomic(LocksHeld(0))),
        ("kernel thread, IRQs masked", ContextInputs { irqs_masked: true, ..sleepable() }, ParkContext::Atomic(IrqsMasked)),
        ("ARM64 boot continuation (idle)", ContextInputs { idle: true, depth: 1, ..sleepable() }, ParkContext::Atomic(Idle)),
        ("before the scheduler dispatches", ContextInputs { depth: 1, ..ContextInputs::default() }, ParkContext::EarlyBoot),
        ("no current thread", ContextInputs { has_thread: false, ..sleepable() }, ParkContext::Atomic(NoThread)),
        ("hard IRQ in a syscall", ContextInputs { hardirq: true, depth: 1, baseline: 1, ..sleepable() }, ParkContext::Atomic(HardIrq)),
        ("softirq on the last release", ContextInputs { softirq: true, ..sleepable() }, ParkContext::Atomic(SoftIrq)),
        ("NMI inside a hard IRQ", ContextInputs { nmi: true, hardirq: true, ..sleepable() }, ParkContext::Atomic(Nmi)),
    ];
    for (name, inputs, expected) in rows {
        assert_eq!(classify(inputs), expected, "{name}");
    }
}

/// Over the whole input space (8 flags, depth 0-3, baseline 0-2): Sleepable
/// exactly when no interrupt context, the scheduler dispatching, a non-idle
/// thread, depth equal to its baseline, and an allowed mask; and each answer
/// survives its receipt code.
#[test]
fn context_is_sleepable_exactly_when_nothing_forbids_it() {
    let mut sleepable_count = 0;
    for flags in 0u32..256 {
        for depth in 0..4 {
            for baseline in 0..3 {
                let bit = |n: u32| flags & (1 << n) != 0;
                let inputs = ContextInputs {
                    dispatching: bit(0),
                    has_thread: bit(1),
                    idle: bit(2),
                    hardirq: bit(3),
                    softirq: bit(4),
                    nmi: bit(5),
                    irqs_masked: bit(6),
                    baseline_irqs_masked: bit(7),
                    depth,
                    baseline,
                };
                let allowed = !inputs.hardirq
                    && !inputs.softirq
                    && !inputs.nmi
                    && inputs.dispatching
                    && inputs.has_thread
                    && !inputs.idle
                    && depth == baseline
                    && (!inputs.irqs_masked || inputs.baseline_irqs_masked);
                let context = classify(inputs);
                assert_eq!(context == ParkContext::Sleepable, allowed, "{inputs:?} -> {context:?}");
                sleepable_count += allowed as u32;
                assert_eq!(ParkContext::from_code(context.code()), Some(context));
                if !inputs.dispatching && !inputs.hardirq && !inputs.softirq && !inputs.nmi {
                    assert_eq!(context, ParkContext::EarlyBoot);
                }
            }
        }
    }
    // Each of the 3 baselines is Sleepable at one depth, under 3 of the 4
    // mask settings (unmasked with either baseline mask; masked with a
    // masked baseline): 9 combinations.
    assert_eq!(sleepable_count, 3 * 3);
}

// -------------------------------------------------------------- the waits

#[derive(Clone, Copy, Debug)]
enum Setup {
    KernelThread,
    Arm64Syscall,
    X86Syscall,
    Braked,
}

fn model_for(setup: Setup, queues: usize) -> Model {
    match setup {
        Setup::KernelThread | Setup::Braked => Model::new(queues, 0),
        Setup::Arm64Syscall => Model::new(queues, 1),
        Setup::X86Syscall => Model::x86_syscall(queues),
    }
}

/// Run one `wait` on queue 0 with the model's deadline, checking the
/// properties each end must have.
fn run_wait(model: &Model, mode: WaitMode, done: impl FnMut() -> bool) -> Result<WaitEnd, ParkError> {
    let (depth, irqs) = (model.depth(), model.irqs());
    let end = wait(model, SITE, &model.queues[0], done, Some(model.deadline_ns), mode);
    check_after(model, depth, irqs);
    end
}

fn check_after(model: &Model, depth: u32, irqs: bool) {
    assert_eq!(model.depth(), depth, "entry depth restored");
    assert_eq!(model.irqs(), irqs, "entry interrupt state restored");
    assert!(!model.linked(), "no queue entry left linked");
    assert_eq!(model.state(), State::Running);
    assert!(matches!(model.word.state().1, Phase::Idle), "the word is finished");
    for event in model.events() {
        if let Event::Sleep { depth, .. } = event {
            assert_eq!(depth, 0, "the switch happens at depth 0");
        }
    }
}

/// The arrival rules E2.2 binds (design §3.3), stated as who makes the
/// ending transition and when, step by step through a wait:
///
/// - a queue wake, task or interrupt context, ends the wait when it arrives;
/// - the timer ends it at the deadline if the thread is still published
///   blocked; otherwise the passed deadline is seen at the next deadline
///   check;
/// - a signal or kill the mode admits is seen at the next signal check;
/// - any arrival makes a blocked thread runnable, so its switch returns, and
///   a switch that returns without an end publishes again and repeats.
///
/// `Err(n)` when iteration `n`'s switch would not return.
fn reference(arrivals: &[(At, Action)], mode: WaitMode) -> Result<Arrival, u32> {
    let admitted = |kill: bool, signal: bool| match mode {
        WaitMode::Interruptible if kill => Some(Arrival::Killed),
        WaitMode::Interruptible if signal => Some(Arrival::Interrupted),
        WaitMode::Killable if kill => Some(Arrival::Killed),
        _ => None,
    };
    let actions_at = |at: At| arrivals.iter().filter(move |(when, _)| *when == at).map(|(_, action)| *action);
    let (kill, signal, passed) = (Cell::new(false), Cell::new(false), Cell::new(false));
    for iteration in 1..=8u32 {
        let mut blocked = true;
        let apply = |action: Action, blocked: &mut bool| -> Option<Arrival> {
            let end = match action {
                Action::Wake(_) | Action::IrqWake(_) => Some(Arrival::Woken),
                Action::Deadline => {
                    passed.set(true);
                    blocked.then_some(Arrival::Timeout)
                }
                Action::Signal => {
                    signal.set(true);
                    None
                }
                Action::Kill => {
                    kill.set(true);
                    None
                }
                Action::StaleInbox | Action::InterruptExit => None,
            };
            if action != Action::InterruptExit {
                *blocked = false;
            }
            end
        };
        // Scripted points fire their actions once, in the first iteration.
        let first = iteration == 1;
        if first {
            for action in actions_at(At::Point(Point::Published)) {
                if let Some(end) = apply(action, &mut blocked) {
                    return Ok(end);
                }
            }
        }
        if let Some(end) = admitted(kill.get(), signal.get()) {
            return Ok(end);
        }
        if first {
            for action in actions_at(At::Point(Point::SignalChecked)) {
                if let Some(end) = apply(action, &mut blocked) {
                    return Ok(end);
                }
            }
        }
        if passed.get() {
            return Ok(Arrival::Timeout);
        }
        if first {
            for point in [Point::DeadlineChecked, Point::Committed] {
                for action in actions_at(At::Point(point)) {
                    if let Some(end) = apply(action, &mut blocked) {
                        return Ok(end);
                    }
                }
            }
        }
        for action in actions_at(At::Asleep(iteration)) {
            if let Some(end) = apply(action, &mut blocked) {
                return Ok(end);
            }
        }
        if blocked {
            return Err(iteration);
        }
    }
    panic!("no end within 8 iterations: {arrivals:?}");
}

fn decode(end: WaitEnd) -> Arrival {
    match end {
        WaitEnd::Woken => Arrival::Woken,
        WaitEnd::Timeout => Arrival::Timeout,
        WaitEnd::Interrupted(errno) => {
            assert_eq!(errno, EINTR);
            Arrival::Interrupted
        }
        WaitEnd::Killed => Arrival::Killed,
        other => panic!("unexpected end {other:?}"),
    }
}

const PLACES: [At; 6] = [
    At::Point(Point::Published),
    At::Point(Point::SignalChecked),
    At::Point(Point::DeadlineChecked),
    At::Point(Point::Committed),
    At::Asleep(1),
    At::Asleep(2),
];

const ACTIONS: [Action; 6] = [Action::Wake(0), Action::IrqWake(0), Action::Signal, Action::Kill, Action::Deadline, Action::Wake(1)];

/// A script whose arrivals cannot end the wait in `mode` leaves the thread
/// switched out. For each switch that would not return, add a
/// wake in that switch.
fn completes(arrivals: &[(At, Action)], mode: WaitMode) -> Vec<(At, Action)> {
    let mut script = arrivals.to_vec();
    while let Err(iteration) = reference(&script, mode) {
        script.push((At::Asleep(iteration), Action::Wake(0)));
    }
    script
}


fn scripted(setup: Setup, queues: usize, script: &[(At, Action)]) -> Model {
    let model = model_for(setup, queues);
    for (at, action) in script {
        model.at(*at, *action);
    }
    model
}

/// Each single arrival and each ordered pair, at each point from publication
/// to after the switch, in each mode and caller shape: the wait ends once,
/// with the first arrival's reason, and each end restores the caller.
#[test]
fn wait_ends_once_with_the_first_arrivals_reason() {
    let modes = [WaitMode::Interruptible, WaitMode::Killable, WaitMode::Uninterruptible];
    let setups = [Setup::KernelThread, Setup::Arm64Syscall, Setup::X86Syscall];
    let mut cases = 0;
    for mode in modes {
        for setup in setups {
            let mut scripts: Vec<Vec<(At, Action)>> = Vec::new();
            for place in PLACES {
                for action in ACTIONS {
                    if action == Action::Wake(1) {
                        continue;
                    }
                    scripts.push(vec![(place, action)]);
                    for later in PLACES.iter().filter(|p| **p != place) {
                        for second in ACTIONS {
                            if second != Action::Wake(1) {
                                scripts.push(vec![(place, action), (*later, second)]);
                            }
                        }
                    }
                }
            }
            for arrivals in scripts {
                let script = completes(&arrivals, mode);
                // Arrivals placed after the end do not run; the reference
                // and the model agree on that because both stop at the end.
                let model = scripted(setup, 1, &script);
                let end = run_wait(&model, mode, || false).expect("sleepable");
                let expected = reference(&script, mode).unwrap();
                assert_eq!(decode(end), expected, "{setup:?} {mode:?} {script:?}");
                cases += 1;
            }
        }
    }
    assert!(cases > 1000, "{cases}");
}

/// The same for `wait_any` over two queues: a wake through either ends it,
/// and the end unlinks it from both.
#[test]
fn wait_any_ends_once_through_either_queue() {
    for mode in [WaitMode::Interruptible, WaitMode::Uninterruptible] {
        for place in PLACES {
            for first in ACTIONS {
                for later in PLACES.iter().filter(|p| **p != place) {
                    for second in [Action::Wake(0), Action::Wake(1), Action::IrqWake(1), Action::Deadline] {
                        let script = completes(&[(place, first), (*later, second)], mode);
                        let model = scripted(Setup::KernelThread, 2, &script);
                        let (depth, irqs) = (model.depth(), model.irqs());
                        let queues: Vec<&EventQueue> = model.queues.iter().collect();
                        let end = wait_any(&model, SITE, &queues, || false, Some(model.deadline_ns), mode).unwrap();
                        check_after(&model, depth, irqs);
                        assert_eq!(decode(end), reference(&script, mode).unwrap(), "{mode:?} {script:?}");
                        // A wake after the end finds nobody on either queue.
                        assert_eq!(model.queues[0].wake_n(&model, 1) + model.queues[1].wake_n(&model, 1), 0);
                    }
                }
            }
        }
    }
}

#[test]
fn a_condition_that_already_holds_publishes_nothing() {
    for setup in [Setup::KernelThread, Setup::Arm64Syscall, Setup::X86Syscall] {
        let model = model_for(setup, 1);
        assert_eq!(run_wait(&model, WaitMode::Interruptible, || true), Ok(WaitEnd::Condition));
        assert_eq!(model.sleeps(), 0);
        assert_eq!(model.becomes_runnable.get(), 0);
    }
    let model = model_for(Setup::KernelThread, 2);
    let queues: Vec<&EventQueue> = model.queues.iter().collect();
    assert_eq!(wait_any(&model, SITE, &queues, || true, None, WaitMode::Killable), Ok(WaitEnd::Condition));
    assert!(!model.linked());
}

#[test]
fn no_memory_for_the_entry_publishes_nothing_and_restores_the_caller() {
    for setup in [Setup::KernelThread, Setup::X86Syscall] {
        let model = model_for(setup, 1);
        let end = with_budget(0, || run_wait(&model, WaitMode::Interruptible, || false));
        assert_eq!(end, Ok(WaitEnd::NoMemory));
        assert_eq!(model.becomes_runnable.get(), 0);
        assert_eq!(model.sleeps(), 0);
    }
}

#[test]
fn a_claimed_kill_refuses_publication_as_killed() {
    let model = model_for(Setup::Arm64Syscall, 1);
    model.set_must_not_sleep();
    assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Killed));
    assert_eq!(model.sleeps(), 0);
}

/// E2.2's one behaviour change: a kernel thread holding a level (depth 1,
/// baseline 0) gets a receipt and an error, and the thread does not publish.
#[test]
fn a_context_holding_a_level_gets_a_receipt_and_an_error() {
    let model = Model::new(1, 0);
    model.set_depth(1);
    let end = run_wait(&model, WaitMode::Interruptible, || false);
    let refused = ParkContext::Atomic(AtomicReason::LocksHeld(1));
    assert_eq!(end, Err(ParkError::NotSleepable(refused)));
    assert_eq!(model.events(), [Event::Receipt(refused)]);
    let model = Model::new(2, 1);
    model.set_depth(2);
    let queues: Vec<&EventQueue> = model.queues.iter().collect();
    assert_eq!(wait_any(&model, SITE, &queues, || false, None, WaitMode::Killable), Err(ParkError::NotSleepable(refused)));
    assert!(!model.linked());
}

/// The same thread under `park::Braked` parks, releases the guard's level
/// for the switch, and is back at depth and baseline 0 when the guard drops.
#[test]
fn braked_raises_depth_and_baseline_together() {
    let model = Model::new(1, 0);
    model.at(At::Asleep(1), Action::Wake(0));
    {
        let _braked = Braked::new(&model);
        assert_eq!(model.depth(), 1);
        assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Woken));
        assert_eq!(model.depth(), 1);
    }
    assert_eq!(model.depth(), 0);
    assert_eq!(crate::park_core::ParkHooks::baseline(&model), 0);
}

/// Publication model (design §3.2): from publishing until the switch, an
/// interrupt exit does not read depth 0, for a baseline-0 waiter as for a
/// braked one.
#[test]
fn an_interrupt_exit_between_publication_and_switch_never_reads_depth_zero() {
    for setup in [Setup::KernelThread, Setup::Arm64Syscall, Setup::X86Syscall, Setup::Braked] {
        let model = model_for(setup, 1);
        for point in [Point::Published, Point::SignalChecked, Point::DeadlineChecked, Point::Committed, Point::Republished] {
            model.at(At::Point(point), Action::InterruptExit);
        }
        model.at(At::Asleep(1), Action::StaleInbox);
        model.at(At::Asleep(2), Action::Wake(0));
        let _braked = matches!(setup, Setup::Braked).then(|| Braked::new(&model));
        assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Woken));
        let exits: Vec<u32> = model
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::InterruptExitDepth(depth) => Some(depth),
                _ => None,
            })
            .collect();
        assert_eq!(exits.len(), 5, "{setup:?}: an exit was sampled at each point, the second pass's publication included");
        assert!(exits.iter().all(|depth| *depth >= 1), "{setup:?}: {exits:?}");
    }
}

/// A wake published by an interrupt waker for one wait, and applied by the
/// inbox drain during the next, does not end the next wait: it sleeps again
/// until its own arrival.
#[test]
fn a_late_wake_from_an_earlier_wait_does_nothing() {
    let model = Model::new(1, 0);
    // The first wait ends by an interrupt wake before it commits; the inbox
    // still holds that wake.
    model.at(At::Point(Point::Published), Action::IrqWake(0));
    assert_eq!(run_wait(&model, WaitMode::Interruptible, || false), Ok(WaitEnd::Woken));
    assert_eq!(model.inbox_len(), 1);
    let first_sleeps = model.sleeps();
    // The second wait's first switch drains it, and no arrival ends the wait
    // until its own deadline.
    model.at(At::Asleep(first_sleeps + 2), Action::Deadline);
    assert_eq!(run_wait(&model, WaitMode::Interruptible, || false), Ok(WaitEnd::Timeout));
    assert_eq!(model.sleeps() - first_sleeps, 2, "the late wake cost one extra switch and no end");
    assert!(model.events().contains(&Event::Point(Point::Republished, 1)));
}

#[test]
fn a_stale_arrival_for_an_earlier_sequence_changes_nothing() {
    let word = WaitWord::new();
    let first = word.begin();
    word.finish(first);
    let second = word.begin();
    assert!(!word.arrive(first, Arrival::Timeout));
    assert!(!word.arrive(first, Arrival::Woken));
    assert_eq!(word.state(), (second, Phase::Parking));
    assert!(word.arrive(second, Arrival::Woken));
    assert!(!word.arrive(second, Arrival::Timeout), "the second arrival does nothing");
    assert_eq!(word.commit(second), Err(Arrival::Woken));
    word.finish(second);
    assert!(!word.arrive_current(Arrival::Woken), "a finished wait takes no arrival");
}

#[test]
fn receipts_keep_the_newest_and_never_block() {
    let ring: ReceiptRing<4> = ReceiptRing::new();
    for n in 0..6u64 {
        ring.record(Receipt { site: n, context: ParkContext::Atomic(AtomicReason::LocksHeld(n as u8)), tid: TID, depth: n as u32 });
    }
    let mut out = [Receipt { site: 0, context: ParkContext::Sleepable, tid: 0, depth: 0 }; 8];
    let n = ring.latest(&mut out);
    assert_eq!(ring.written(), 6);
    assert_eq!(n, 4);
    assert_eq!(out[..4].iter().map(|r| r.site).collect::<Vec<_>>(), [5, 4, 3, 2]);
    assert_eq!(out[0].context, ParkContext::Atomic(AtomicReason::LocksHeld(5)));
    assert_eq!(out[0].depth, 5);
}

// ---------------------------------------------------- E2.3: the interrupt exit

/// I4 (design §3.5): a wait that is published but not committed is
/// runnable at an involuntary switch. An exit with a reschedule pending
/// switches away from it without switching it out as blocked, and an exit
/// without one returns to it, so it runs on to its own commit.
#[test]
fn an_interrupt_exit_leaves_a_published_uncommitted_wait_runnable() {
    let parking = ExitThread::BlockedOnIo(Phase::Parking);
    assert_ne!(
        exit_switch(parking, true),
        ExitSwitch::SwitchOut,
        "preempted after publishing and before committing, the waiter was switched out as blocked"
    );
    assert_eq!(exit_switch(parking, false), ExitSwitch::Resume, "with no reschedule pending, the waiter runs on");
}

/// The rest of the decision: a committed wait, a wait not on `park::wait`
/// (its word idle), a wait whose word has ended (its scheduler wake follows),
/// another blocked state and an exited thread are switched out; a running
/// thread is preempted only on a reschedule.
#[test]
fn an_interrupt_exit_switches_out_what_may_not_run() {
    use ExitThread::*;
    for need_resched in [false, true] {
        for thread in [BlockedOnIo(Phase::Parked), BlockedOnIo(Phase::Idle), BlockedOnIo(Phase::Runnable(Arrival::Woken)), Blocked, Terminated] {
            assert_eq!(exit_switch(thread, need_resched), ExitSwitch::SwitchOut, "{thread:?}, need_resched {need_resched}");
        }
    }
    assert_eq!(exit_switch(Runnable, true), ExitSwitch::Preempt);
    assert_eq!(exit_switch(Runnable, false), ExitSwitch::Resume);
}

/// I4 through production `wait`: an interrupt exit at each of the four
/// points between publication and the commit, in each of the four caller
/// shapes, does not switch the waiter out while its wait is uncommitted.
/// (E2.2 publishes at depth 1, so those exits did not switch at all; E2.3
/// publishes at the baseline.)
#[test]
fn an_interrupt_exit_before_the_commit_does_not_switch_the_wait_out() {
    for setup in [Setup::KernelThread, Setup::Arm64Syscall, Setup::X86Syscall, Setup::Braked] {
        let model = model_for(setup, 1);
        for point in [Point::Published, Point::SignalChecked, Point::DeadlineChecked, Point::Republished] {
            model.at(At::Point(point), Action::InterruptExit);
        }
        model.at(At::Asleep(1), Action::StaleInbox);
        model.at(At::Asleep(2), Action::Wake(0));
        let _braked = matches!(setup, Setup::Braked).then(|| Braked::new(&model));
        assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Woken), "{setup:?}");
        let switched: Vec<Phase> = model
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::SwitchedOut(phase) => Some(phase),
                _ => None,
            })
            .collect();
        assert!(!switched.contains(&Phase::Parking), "{setup:?}: switched out before its commit: {switched:?}");
    }
}

/// Publish, an involuntary switch, a wake, then the commit: the wait ends
/// woken without sleeping.
#[test]
fn a_wake_after_an_involuntary_switch_ends_the_wait_at_its_commit() {
    let model = Model::new(1, 0);
    model.at(At::Point(Point::Published), Action::InterruptExit);
    model.at(At::Point(Point::SignalChecked), Action::Wake(0));
    assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Woken));
    assert_eq!(model.sleeps(), 0, "the wait ended at its commit");
}

/// Publish, an involuntary switch, the commit, then a wake: the wait ends
/// woken, once.
#[test]
fn a_wake_after_the_commit_ends_a_wait_switched_before_it_once() {
    let model = Model::new(1, 0);
    model.at(At::Point(Point::Published), Action::InterruptExit);
    model.at(At::Point(Point::Committed), Action::Wake(0));
    model.at(At::Point(Point::Committed), Action::Wake(0));
    assert_eq!(run_wait(&model, WaitMode::Uninterruptible, || false), Ok(WaitEnd::Woken));
    assert_eq!(model.sleeps(), 1);
}
