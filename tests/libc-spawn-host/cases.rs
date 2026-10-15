use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::CStr;

type Invocation = (u64, [u64; 6], Vec<u8>);
thread_local! {
    static CALLS: RefCell<Vec<Invocation>> = const { RefCell::new(Vec::new()) };
    static RESULTS: RefCell<VecDeque<i64>> = const { RefCell::new(VecDeque::new()) };
    static ALLOCATIONS: RefCell<usize> = const { RefCell::new(0) };
}
pub mod mock_raw {
    use super::*;
    pub unsafe fn syscall6(n: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
        let name = if n == nr::EXEC || n == nr::CHDIR {
            a
        } else if n == nr::OPENAT {
            b
        } else {
            0
        };
        let bytes = if n == nr::SIGPROCMASK && b != 0 {
            (*(b as *const u64)).to_ne_bytes().to_vec()
        } else if name == 0 {
            Vec::new()
        } else {
            CStr::from_ptr(name as *const i8).to_bytes().to_vec()
        };
        CALLS.with(|v| v.borrow_mut().push((n, [a, b, c, d, e, f], bytes)));
        if n == nr::SIGPROCMASK && c != 0 {
            *(c as *mut u64) = 0x1234;
        }
        if n == nr::SIGACTION && c != 0 {
            *(c as *mut u64) = match a {
                10 => 0x100,
                12 => 1,
                _ => 0,
            };
        }
        RESULTS.with(|v| v.borrow_mut().pop_front().expect("unexpected syscall")) as u64
    }
}
unsafe fn malloc(n: usize) -> *mut u8 {
    ALLOCATIONS.with(|a| *a.borrow_mut() += 1);
    let layout = std::alloc::Layout::from_size_align(n + 16, 16).unwrap();
    let p = std::alloc::alloc(layout);
    p.cast::<usize>().write(n);
    p.add(16)
}
unsafe fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    ALLOCATIONS.with(|a| *a.borrow_mut() -= 1);
    let base = p.sub(16);
    let n = base.cast::<usize>().read();
    std::alloc::dealloc(
        base,
        std::alloc::Layout::from_size_align(n + 16, 16).unwrap(),
    );
}
mod env {
    pub unsafe fn getenv(_: *const u8) -> *const u8 {
        core::ptr::null()
    }
}
mod conf {
    pub fn system_value(_: i32) -> i64 {
        1024
    }
}
fn setup(results: &[i64]) {
    RESULTS.with(|v| *v.borrow_mut() = results.iter().copied().collect());
    CALLS.with(|v| v.borrow_mut().clear());
}
fn calls() -> Vec<Invocation> {
    assert!(RESULTS.with(|v| v.borrow().is_empty()), "missing syscalls");
    CALLS.with(|v| v.borrow().clone())
}

#[test]
fn attributes_roundtrip_and_reject_invalid_flags_without_changing_them() {
    unsafe {
        let mut attr = Attr::default();
        assert_eq!(posix_spawnattr_init(&mut attr), 0);
        let mut flags = -1;
        posix_spawnattr_getflags(&attr, &mut flags);
        assert_eq!(flags, 0);
        assert_eq!(
            posix_spawnattr_setflags(&mut attr, SETSIGMASK | SETSIGDEF),
            0
        );
        assert_eq!(posix_spawnattr_setflags(&mut attr, 128), EINVAL);
        assert_eq!(
            posix_spawnattr_setflags(&mut attr, SETSID | SETPGROUP),
            EINVAL
        );
        assert_eq!(attr.flags, SETSIGMASK | SETSIGDEF);
        let mut group = 0;
        posix_spawnattr_setpgroup(&mut attr, 21);
        posix_spawnattr_getpgroup(&attr, &mut group);
        assert_eq!(group, 21);
        let mut mask = 0;
        posix_spawnattr_setsigmask(&mut attr, &123);
        posix_spawnattr_getsigmask(&attr, &mut mask);
        assert_eq!(mask, 123);
        posix_spawnattr_setsigdefault(&mut attr, &456);
        posix_spawnattr_getsigdefault(&attr, &mut mask);
        assert_eq!(mask, 456);
        let mut priority = 0;
        posix_spawnattr_setschedparam(&mut attr, &5);
        posix_spawnattr_getschedparam(&attr, &mut priority);
        assert_eq!(priority, 5);
        assert_eq!(posix_spawnattr_setschedpolicy(&mut attr, 2), 0);
        assert_eq!(posix_spawnattr_setschedpolicy(&mut attr, 3), EINVAL);
        posix_spawnattr_getschedpolicy(&attr, &mut priority);
        assert_eq!(priority, 2);
        assert_eq!(posix_spawnattr_destroy(&mut attr), 0);
    }
}

#[test]
fn file_actions_copy_paths_preserve_order_and_release_storage() {
    unsafe {
        let mut actions = FileActions {
            head: null_mut(),
            tail: null_mut(),
        };
        posix_spawn_file_actions_init(&mut actions);
        let mut path = *b"work\0";
        assert_eq!(
            posix_spawn_file_actions_addchdir(&mut actions, path.as_ptr()),
            0
        );
        path[0] = b'X';
        assert_eq!(
            posix_spawn_file_actions_addopen(&mut actions, 1, b"out\0".as_ptr(), 0x241, 0o640),
            0
        );
        assert_eq!(posix_spawn_file_actions_adddup2(&mut actions, 1, 2), 0);
        assert_eq!(posix_spawn_file_actions_addclose(&mut actions, 7), 0);
        assert_eq!(posix_spawn_file_actions_addfchdir(&mut actions, 8), 0);
        setup(&[0, -9, 3, 1, 0, 2, 0, 0]);
        assert_eq!(file_actions(&actions), Ok(()));
        let c = calls();
        assert_eq!(
            c.iter().map(|c| c.0).collect::<Vec<_>>(),
            [
                nr::CHDIR,
                nr::CLOSE,
                nr::OPENAT,
                nr::DUP3,
                nr::CLOSE,
                nr::DUP3,
                nr::CLOSE,
                nr::FCHDIR
            ]
        );
        assert_eq!(c[0].2, b"work");
        assert_eq!(c[2].2, b"out");
        assert_eq!(c[2].1[2..4], [0x241, 0o640]);
        posix_spawn_file_actions_destroy(&mut actions);
        assert!(actions.head.is_null() && actions.tail.is_null());
        assert_eq!(ALLOCATIONS.with(|a| *a.borrow()), 0);
    }
}

#[test]
fn invalid_action_descriptors_do_not_append_or_allocate() {
    unsafe {
        let mut actions = FileActions {
            head: null_mut(),
            tail: null_mut(),
        };
        assert_eq!(posix_spawn_file_actions_addclose(&mut actions, -1), EBADF);
        assert_eq!(
            posix_spawn_file_actions_addopen(&mut actions, 1024, b"x\0".as_ptr(), 0, 0),
            EBADF
        );
        assert_eq!(
            posix_spawn_file_actions_adddup2(&mut actions, 1, 1024),
            EBADF
        );
        assert!(actions.head.is_null());
    }
}

#[test]
fn dup_same_descriptor_clears_cloexec_and_action_failure_stops_execution() {
    unsafe {
        setup(&[5, 0]);
        assert_eq!(duplicate(4, 4), Ok(()));
        let c = calls();
        assert_eq!(c[0].1[..3], [4, 1, 0]);
        assert_eq!(c[1].1[..3], [4, 2, 4]);
        let mut actions = FileActions {
            head: null_mut(),
            tail: null_mut(),
        };
        posix_spawn_file_actions_adddup2(&mut actions, 4, 5);
        posix_spawn_file_actions_addclose(&mut actions, 6);
        setup(&[-9]);
        assert_eq!(file_actions(&actions), Err(EBADF));
        assert_eq!(calls().len(), 1);
        posix_spawn_file_actions_destroy(&mut actions);
    }
}

#[test]
fn path_search_continues_missing_and_denied_but_never_runs_a_shell() {
    unsafe {
        setup(&[-13, -20, -2]);
        assert_eq!(
            execute(b"app\0".as_ptr(), null(), null(), Some(b"/one::/two")),
            Err(EACCES)
        );
        assert_eq!(
            calls().iter().map(|c| c.2.clone()).collect::<Vec<_>>(),
            [
                b"/one/app".to_vec(),
                b"./app".to_vec(),
                b"/two/app".to_vec()
            ]
        );
        setup(&[-8]);
        assert_eq!(
            execute(b"app\0".as_ptr(), null(), null(), Some(b"/one:/two")),
            Err(8)
        );
        assert_eq!(calls().len(), 1);
        setup(&[-12]);
        assert_eq!(
            execute(b"./app\0".as_ptr(), null(), null(), Some(b"/one")),
            Err(12)
        );
        assert_eq!(calls()[0].2, b"./app");
    }
}

#[test]
fn parent_restores_mask_and_returns_fork_error_or_waitable_pid() {
    unsafe {
        let mut pid = 99;
        setup(&[0, -11, 0]);
        assert_eq!(
            posix_spawn(&mut pid, b"app\0".as_ptr(), null(), null(), null(), null()),
            11
        );
        assert_eq!(pid, 99);
        let c = calls();
        assert_eq!(c[0].0, nr::SIGPROCMASK);
        assert_eq!(c[2].0, nr::SIGPROCMASK);
        assert_eq!(c[2].2, 0x1234u64.to_ne_bytes());
        setup(&[0, 42, 0]);
        assert_eq!(
            posix_spawnp(&mut pid, b"app\0".as_ptr(), null(), null(), null(), null()),
            0
        );
        assert_eq!(pid, 42);
        assert_eq!(calls().len(), 3);
        setup(&[0, 43, 0]);
        assert_eq!(
            posix_spawn(
                null_mut(),
                b"app\0".as_ptr(),
                null(),
                null(),
                null(),
                null()
            ),
            0
        );
        calls();
    }
}

fn signal_results(reset_ignored: bool) -> Vec<i64> {
    let mut results = Vec::new();
    for sig in 1..=64 {
        if sig == 9 || sig == 19 {
            continue;
        }
        results.push(0);
        if sig == 10 || reset_ignored && sig == 12 {
            results.push(0);
        }
    }
    results
}

#[test]
fn scheduler_precedence_and_signal_dispositions_are_applied_before_exec() {
    unsafe {
        let attr = Attr {
            flags: SETSCHEDULER | SETSCHEDPARAM | SETSIGDEF,
            sigdefault: 1 << 11,
            ..Attr::default()
        };
        let mut results = vec![0];
        results.extend(signal_results(true));
        setup(&results);
        assert_eq!(attributes(&attr), Ok(()));
        let c = calls();
        assert_eq!(c[0].0, nr::SCHED_SETSCHEDULER);
        assert!(!c.iter().any(|c| c.0 == nr::SCHED_SETPARAM));
        assert_eq!(
            c.iter()
                .filter(|c| c.0 == nr::SIGACTION && c.1[1] != 0)
                .map(|c| c.1[0])
                .collect::<Vec<_>>(),
            [10, 12]
        );
    }
}

#[test]
fn child_applies_requested_mask_and_passes_argv_envp_to_exec() {
    unsafe {
        let attr = Attr {
            flags: SETSIGMASK,
            sigmask: 0x5678,
            ..Attr::default()
        };
        let argv = [b"app\0".as_ptr(), null()];
        let envp = [b"NAME=value\0".as_ptr(), null()];
        let mut results = vec![0, 0];
        results.extend(signal_results(false));
        results.extend([0, -8]);
        setup(&results);
        let failed = std::panic::catch_unwind(|| {
            spawn(
                null_mut(),
                b"./app\0".as_ptr(),
                null(),
                &attr,
                argv.as_ptr(),
                envp.as_ptr(),
                false,
            )
        });
        assert_eq!(*failed.unwrap_err().downcast::<i32>().unwrap(), 127);
        let c = calls();
        assert_eq!(c[c.len() - 2].2, 0x5678u64.to_ne_bytes());
        let last = c.last().unwrap();
        assert_eq!(last.0, nr::EXEC);
        assert_eq!(last.1[1..3], [argv.as_ptr() as u64, envp.as_ptr() as u64]);
        assert_eq!(last.2, b"./app");
    }
}

#[test]
fn child_failure_exits_127_instead_of_returning_to_application() {
    unsafe {
        setup(&[0, 0, -22]); // block, fork child, first child attribute syscall fails
        let failed = std::panic::catch_unwind(|| {
            spawn(
                null_mut(),
                b"app\0".as_ptr(),
                null(),
                null(),
                null(),
                null(),
                false,
            )
        });
        assert_eq!(*failed.unwrap_err().downcast::<i32>().unwrap(), 127);
        calls();
    }
}

#[test]
fn resetids_uses_guest_syscall_numbers_and_preserves_real_ids() {
    let ids = if GUEST_ARCH == "aarch64" { [174, 176, 143, 145] } else { [102, 104, 114, 113] };
    let mut results = vec![42, 43, 0, 0];
    // sigaction queries plus the inherited caught handler reset.
    results.extend(vec![0; 63]);
    setup(&results);
    unsafe { assert_eq!(attributes(&Attr { flags: RESETIDS, ..Attr::default() }), Ok(())); }
    let c = calls();
    assert_eq!(c[..4].iter().map(|c| c.0).collect::<Vec<_>>(), ids);
    assert_eq!(c[2].1, [u64::MAX, 43, 0, 0, 0, 0]);
    assert_eq!(c[3].1, [u64::MAX, 42, 0, 0, 0, 0]);
}
