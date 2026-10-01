//! Exercise the production x86 idle wait and admission function with simulated
//! IRQ delivery. A real boot separately checks STI/HLT and disk IRQ delivery.
use std::{fs, process::Command};

#[test]
fn idle_completion_and_schedule_admission() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let completion = fs::read_to_string(root.join("kernel/src/task/completion.rs")).unwrap();
    let wait = completion
        .split("fn wait_idle_completion(")
        .nth(1)
        .expect("idle wait")
        .split("/// Completion primitive")
        .next()
        .unwrap();
    let per_cpu = fs::read_to_string(root.join("kernel/src/per_cpu.rs")).unwrap();
    let admission = per_cpu
        .split("pub fn can_schedule(")
        .nth(1)
        .expect("schedule admission")
        .split("/// Get per-CPU base address")
        .next()
        .unwrap();
    let source =
        format!("{HARNESS}\nfn wait_idle_completion({wait}\npub fn can_schedule({admission}");
    let dir = std::env::temp_dir().join(format!("idle-completion-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("test.rs");
    let binary = dir.join("test");
    fs::write(&input, source).unwrap();
    let compile = Command::new("rustc")
        .args(["--edition=2021", "--test", "-Dwarnings"])
        .arg(input)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(binary).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    print!("{}", String::from_utf8_lossy(&run.stdout));
}

const HARNESS: &str = r#"
extern crate self as x86_64;
extern crate self as log;
use std::{cell::RefCell, collections::VecDeque, sync::{Arc, atomic::{AtomicU32, Ordering}}};
#[macro_export]
macro_rules! warn { ($($arg:tt)*) => { let _ = format_args!($($arg)*); }; }
struct Completion { done: Arc<AtomicU32> }
struct State {
    irqs: bool, preempt: u32, initial_preempt: u32, now: u64,
    done: Arc<AtomicU32>, events: VecDeque<Option<u32>>, halts: usize,
    current: Option<u64>, idle: u64, exception: bool, resched: bool,
    thread: task::thread::Thread,
}
impl Default for State {
    fn default() -> Self {
        Self {
            irqs: true, preempt: 0, initial_preempt: 0, now: 0,
            done: Arc::new(AtomicU32::new(0)), events: VecDeque::new(), halts: 0,
            current: Some(1), idle: 1, exception: false, resched: true,
            thread: task::thread::Thread { state: task::thread::ThreadState::Running },
        }
    }
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
pub mod instructions {
    pub mod interrupts {
        pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
            let old = super::super::STATE.with(|s| { let mut s = s.borrow_mut(); let old = s.irqs; s.irqs = false; old });
            let result = f();
            super::super::STATE.with(|s| s.borrow_mut().irqs = old);
            result
        }
        pub fn enable_and_hlt() {
            super::super::STATE.with(|s| {
                let mut s = s.borrow_mut();
                assert!(!s.irqs, "completion check must run with IRQs masked");
                assert!(s.preempt > s.initial_preempt, "idle must hold its own scheduling brake");
                s.irqs = true;
                s.halts += 1;
                assert!(s.halts < 4, "wait failed to finish");
                s.now += 1;
                if let Some(Some(token)) = s.events.pop_front() { s.done.store(token, super::super::Ordering::Release); }
            });
        }
        pub fn disable() { super::super::STATE.with(|s| s.borrow_mut().irqs = false); }
    }
    // Existing admission diagnostics are irrelevant to the simulated CPU.
    pub mod port {
        pub struct Port<T>(std::marker::PhantomData<T>);
        impl<T> Port<T> {
            pub fn new(_address: u16) -> Self { Self(std::marker::PhantomData) }
            pub unsafe fn write(&mut self, _value: T) {}
        }
    }
}
mod per_cpu {
    pub fn preempt_disable() { super::STATE.with(|s| s.borrow_mut().preempt += 1); }
    pub fn preempt_enable() {
        super::STATE.with(|s| { let mut s = s.borrow_mut(); assert!(!s.irqs); s.preempt -= 1; });
    }
}
mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        super::STATE.with(|s| { let s = s.borrow(); assert!(!s.irqs); (0, s.now) })
    }
}
fn preempt_count() -> u32 { STATE.with(|s| s.borrow().preempt) }
fn current_thread() -> Option<u64> { STATE.with(|s| s.borrow().current) }
fn in_exception_cleanup_context() -> bool { STATE.with(|s| s.borrow().exception) }
mod task {
    pub mod thread {
        #[derive(Clone, Copy, PartialEq)]
        pub enum ThreadState { Running, BlockedOnSignal, BlockedOnChildExit, BlockedOnTimer, Blocked, Terminated }
        pub struct Thread { pub state: ThreadState }
    }
    pub mod scheduler {
        pub struct Scheduler<'a>(&'a mut super::super::State);
        impl Scheduler<'_> {
            pub fn idle_thread(&self) -> u64 { self.0.idle }
            pub fn current_thread_mut(&mut self) -> Option<&mut super::thread::Thread> {
                if self.0.current.is_some() { Some(&mut self.0.thread) } else { None }
            }
        }
        pub fn with_scheduler<T>(f: impl FnOnce(&mut Scheduler<'_>) -> T) -> Option<T> {
            super::super::STATE.with(|s| Some(f(&mut Scheduler(&mut s.borrow_mut()))))
        }
        pub fn current_thread_id() -> Option<u64> { super::super::current_thread() }
        pub fn is_need_resched() -> bool { super::super::STATE.with(|s| s.borrow().resched) }
    }
}
fn wait_case(preempt: u32, irqs: bool, done: u32, events: &[Option<u32>], timeout: u64, expected: bool, halts: usize) {
    STATE.with(|s| {
        *s.borrow_mut() = State { preempt, initial_preempt: preempt, irqs, events: events.iter().copied().collect(), ..State::default() };
        s.borrow().done.store(done, Ordering::Release);
    });
    let completion = STATE.with(|s| Completion { done: s.borrow().done.clone() });
    assert_eq!(wait_idle_completion(&completion, 7, timeout), expected);
    STATE.with(|s| {
        let s = s.borrow();
        assert_eq!(s.preempt, preempt, "caller preemption count changed");
        assert_eq!(s.irqs, irqs, "caller IRQ state changed");
        assert_eq!(s.halts, halts);
        assert!(s.thread.state == task::thread::ThreadState::Running);
    });
}
#[test]
fn completion_preserves_count_and_irq_state() {
    for count in [0, 1, 3] {
        for irqs in [false, true] {
            wait_case(count, irqs, 0, &[Some(7)], 10, true, 1);
        }
    }
}
#[test]
fn stale_token_and_spurious_irq_do_not_finish_the_wait() {
    wait_case(1, true, 6, &[None, Some(6), Some(7)], 10, true, 3);
}
#[test]
fn timeout_preserves_count_and_irq_state() {
    for irqs in [false, true] {
        wait_case(2, irqs, 0, &[None, Some(6)], 2, false, 2);
        wait_case(2, irqs, 0, &[], 0, false, 0);
    }
}
#[test]
fn completed_token_wins_over_expired_deadline() {
    wait_case(1, true, 7, &[], 0, true, 0);
    wait_case(1, true, 0, &[Some(7)], 1, true, 1);
}
#[test]
fn held_kernel_count_blocks_all_admission_arms() {
    use task::thread::ThreadState::*;
    for state in [Running, BlockedOnSignal, BlockedOnChildExit, BlockedOnTimer, Blocked, Terminated] {
        for count in [1, 3, 0x100, 0x10000] {
            STATE.with(|s| { let mut s = s.borrow_mut(); *s = State::default(); s.thread.state = state; s.exception = true; s.preempt = count; });
            assert!(!can_schedule(0x08));
        }
    }
}
#[test]
fn released_kernel_count_and_userspace_keep_existing_admission() {
    STATE.with(|s| *s.borrow_mut() = State::default());
    assert!(can_schedule(0x08));
    assert!(can_schedule(0x33));
    STATE.with(|s| { let mut s = s.borrow_mut(); s.preempt = 1; s.exception = true; });
    assert!(can_schedule(0x33));
    STATE.with(|s| { let mut s = s.borrow_mut(); s.preempt = 0; s.current = None; });
    assert!(!can_schedule(0x08));
}
"#;
