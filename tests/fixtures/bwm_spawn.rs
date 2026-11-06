// Host support for the production slices compiled by bwm_shutdown.rs.
// Signal generation, masks, dispositions and delivery selection are production
// code; the fake replaces the loader, storage, clock and non-signal attributes.
#![feature(allocator_api)]
#![allow(dead_code)] // Production modules expose APIs outside this scoped check.
extern crate alloc;
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

mod memory {
    pub mod slab {
        pub struct Slab;
        impl Slab { pub fn alloc(&self) -> Option<*mut u8> { None } }
        pub static SIGNAL_HANDLERS_SLAB: Slab = Slab;
        pub struct SlabBox<T>(Box<T>);
        impl<T> SlabBox<T> {
            pub fn from_box(value: Box<T>) -> Self { Self(value) }
            pub unsafe fn from_slab(_: *mut T, _: &Slab) -> Self { unreachable!() }
        }
        impl<T> std::ops::Deref for SlabBox<T> {
            type Target = T;
            fn deref(&self) -> &T { &self.0 }
        }
        impl<T> std::ops::DerefMut for SlabBox<T> {
            fn deref_mut(&mut self) -> &mut T { &mut self.0 }
        }
        impl Clone for SlabBox<crate::signal::types::SignalTable> {
            fn clone(&self) -> Self {
                // SignalTable contains only two arrays of Copy ABI records.
                Self(Box::new(unsafe { std::ptr::read(&**self) }))
            }
        }
    }
}
extern crate self as spin;
#[derive(Default)] pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
}
mod ipc { pub mod fd { pub struct FdTableRef; } }
mod time { pub fn get_monotonic_time_ns() -> (u64, u64) { (0, 0) } }
mod signal {
    #[path = "@ROOT@/kernel/src/signal/constants.rs"] pub mod constants;
    #[path = "@ROOT@/kernel/src/signal/timer_signal.rs"] mod timer_signal;
    pub mod timers {
        pub use super::timer_signal::TimerSignal;
        pub const SI_TIMER: i32 = -2;
        #[derive(Default)] pub struct PosixTimers;
        impl PosixTimers {
            pub fn take_next_due(&self, _: &super::types::ThreadSignals)
                -> Option<(u32, super::types::SigInfo, bool, std::sync::Arc<TimerSignal>)> { None }
            pub fn try_disarm_all(&self) {}
            pub fn is_active(&self) -> bool { false }
        }
    }
    #[path = "@ROOT@/kernel/src/signal/types.rs"] pub mod types;
}
use signal::{constants::*, types::{SignalState, SignalAction, SignalDefaultAction, default_action}};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] struct ProcessId(u64);
impl ProcessId { fn as_u64(self) -> u64 { self.0 } }
#[derive(Clone)] struct Limits;
impl Limits { fn inherit(&self) -> Self { self.clone() } fn get(&self, _: u32) -> Limit { Limit { soft: 100 } } }
struct Limit { soft: u64 }
struct Cwd;
impl Cwd { fn copy(&self) -> Self { Self } }
struct Fds;
impl Fds { fn set_limit(&mut self, _: u64) {} }
struct Thread { resource_limits: Option<Limits> }
struct Process {
    signals: SignalState, pgid: ProcessId, cwd: Cwd, cred: (), nice: i32,
    limits: Limits, exe_path: Option<Arc<Vec<u8>>>, parent: Option<ProcessId>,
    fd_table: Fds, main_thread: Option<Thread>, children: Vec<ProcessId>,
}
impl Process {
    fn loaded() -> Self {
        Self { signals: @SIGNALS@, pgid: ProcessId(1), cwd: Cwd, cred: (), nice: 0,
            limits: Limits, exe_path: None, parent: None, fd_table: Fds,
            main_thread: None, children: Vec::new() }
    }
}
struct Rows(BTreeMap<ProcessId, Process>);
impl Rows {
    fn live_row(&self, pid: &ProcessId) -> Option<&Process> { self.0.get(pid) }
    fn live_row_mut(&mut self, pid: &ProcessId) -> Option<&mut Process> { self.0.get_mut(pid) }
}
mod process {
    pub mod limits {
        pub const NOFILE: u32 = 7;
        pub fn fork_allowed(_: &super::super::Manager, _: super::super::ProcessId) -> bool { true }
    }
}
#[macro_export] macro_rules! diag_println { ($($arg:tt)*) => { { let _ = format_args!($($arg)*); } }; }
struct Manager { processes: Rows, arm: bool, events: Vec<&'static str> }
impl Manager {
    fn create_process_with_argv(&mut self, _: String, _: &[u8], _: &[&[u8]], _: &(), _: Option<ProcessId>)
        -> Result<ProcessId, &'static str> {
        self.events.push("load-child");
        self.processes.0.insert(ProcessId(2), Process::loaded());
        Ok(ProcessId(2))
    }
    @SPAWN_X86@
    @SPAWN_ARM@
}
thread_local! { static KERNEL: RefCell<Option<Manager>> = const { RefCell::new(None) }; }
mod libbreenix {
    pub mod error {
        #[derive(Debug)] pub enum Error { Os(i32) }
        impl Error { pub fn from_syscall(value: i64) -> Result<u64, Self> { if value < 0 { Err(Self::Os((-value) as i32)) } else { Ok(value as u64) } } }
    }
    pub mod process {
        use super::error::Error;
        pub struct Pid(u64);
        impl Pid { fn from_raw(value: u64) -> Self { Self(value) } pub fn raw(self) -> u64 { self.0 } }
        mod nr { pub const SPAWN: u64 = 0x1234; }
        mod raw {
            pub unsafe fn syscall2(nr: u64, path: u64, argv: u64) -> u64 {
                assert_eq!(nr, super::nr::SPAWN);
                assert_eq!(std::ffi::CStr::from_ptr(path as *const i8).to_bytes(), b"/usr/bin/bterm");
                assert_eq!(argv, 0);
                crate::KERNEL.with(|kernel| {
                    let mut kernel = kernel.borrow_mut();
                    let m = kernel.as_mut().unwrap();
                    m.events.push("spawn-syscall");
                    // This must still be the blocked compositor at the syscall.
                    let parent = m.processes.live_row(&crate::ProcessId(1)).unwrap();
                    assert!(parent.signals.is_blocked(crate::SIGTERM));
                    assert!(parent.signals.get_handler(crate::SIGTERM).is_handler());
                    let args = (crate::ProcessId(1), "bterm".into(), &[][..], &[][..], None);
                    let pid = if m.arm { m.spawn_arm(args.0, args.1, args.2, args.3, args.4) }
                        else { m.spawn_process(args.0, args.1, args.2, args.3, args.4) }.unwrap();
                    pid.as_u64()
                })
            }
        }
        @WRAPPER@
    }
    pub mod ui_clock { pub fn now_ns() -> u64 { 10 } }
}
#[path = "@ROOT@/userspace/programs/src/bwm/children.rs"] mod children;
enum HotkeyAction { Exec(Vec<u8>), Close, FocusNext, FocusPrev }
struct Window { chromeless: bool, title_len: usize, title: [u8; 1] }
struct Stack;
enum Input { Close }
fn send_to(_: &[Window], _: usize, _: Input) { panic!("unexpected close") }
fn focus_index(_: &[Window], _: &Stack) -> usize { 0 }
fn show_error(_: &[u8], _: children::LaunchError) { panic!("unexpected launch error") }
@HOTKEY@

#[test]
fn child_term_is_deliverable_and_default_while_parent_remains_blocked() {
    for arm in [false, true] {
        let mut parent = Process::loaded();
        parent.signals.block_signals(sig_mask(SIGTERM));
        parent.signals.set_handler(SIGTERM, SignalAction { handler: 0x1000, ..SignalAction::default() });
        parent.signals.set_pending(SIGTERM);
        assert_eq!(parent.signals.next_deliverable_signal(), None);
        let rows = BTreeMap::from([(ProcessId(1), parent)]);
        KERNEL.with(|k| *k.borrow_mut() = Some(Manager { processes: Rows(rows), arm, events: Vec::new() }));
        let mut children = children::Children::default();
        execute_hotkey_action(&HotkeyAction::Exec(b"/usr/bin/bterm".to_vec()), &[], &Stack, &mut children);
        assert!(!children.is_empty());
        KERNEL.with(|k| {
            let mut k = k.borrow_mut();
            let m = k.as_mut().unwrap();
            assert_eq!(m.events, ["spawn-syscall", "load-child"]);
            let child = m.processes.live_row_mut(&ProcessId(2)).unwrap();
            assert_eq!(child.signals.blocked(), 0);
            assert!(child.signals.get_handler(SIGTERM).is_default());
            child.signals.set_pending(SIGTERM);
            assert_eq!(child.signals.next_deliverable_signal(), Some(SIGTERM));
            assert_eq!(default_action(SIGTERM), SignalDefaultAction::Terminate);
            let parent = m.processes.live_row(&ProcessId(1)).unwrap();
            assert!(parent.signals.is_blocked(SIGTERM));
            assert!(parent.signals.get_handler(SIGTERM).is_handler());
            assert_eq!(parent.signals.next_deliverable_signal(), None);
        });
    }
}
