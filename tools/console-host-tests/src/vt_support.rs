// Exercise the production VT state machine with host scheduler/device stubs.
use std::sync::atomic::AtomicBool;

pub mod process {
    pub struct ProcessId;
    impl ProcessId {
        pub fn new(_: u64) -> Self {
            Self
        }
    }
}
pub mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        (0, 0)
    }
}
pub mod per_cpu {
    pub fn in_interrupt() -> bool {
        false
    }
}
pub mod task {
    pub mod thread {
        pub enum ThreadState {
            BlockedOnIO,
        }
    }
    pub mod scheduler {
        pub fn yield_current() {}
    }
    pub mod waitqueue {
        #[derive(PartialEq)]
        pub enum PrepareOutcome {
            Mismatch,
        }
        pub struct WaitQueueHead;
        impl WaitQueueHead {
            pub const fn new() -> Self {
                Self
            }
            pub fn wake_up(&self) {}
            pub fn finish_wait(&self) {}
            pub fn prepare_to_wait_checked(
                &self,
                _: super::thread::ThreadState,
                _: Option<u64>,
                _: impl FnOnce() -> bool,
            ) -> PrepareOutcome {
                PrepareOutcome::Mismatch
            }
        }
    }
}
pub mod syscall {
    pub mod graphics {
        use std::sync::atomic::AtomicU64;
        pub static DISPLAY_OWNER_PID: AtomicU64 = AtomicU64::new(u64::MAX);
        pub static DISPLAY_OWNER_GROUP: AtomicU64 = AtomicU64::new(u64::MAX);
    }
    pub mod signal {
        pub fn signal_process_from_kernel(_: crate::process::ProcessId, _: u32) {}
    }
    pub mod blocking_io {
        pub fn wait_prepared(
            _: &crate::task::waitqueue::WaitQueueHead,
            _: crate::task::waitqueue::PrepareOutcome,
        ) -> Result<(), i32> {
            Ok(())
        }
    }
}

// The full production module includes device-facing APIs unused by these state tests.
#[allow(dead_code)]
#[path = "../../../kernel/src/console/vt.rs"]
mod vt;
pub static REDRAW: AtomicBool = AtomicBool::new(false);
pub static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);
pub struct Console {
    vts: [vt::Vt; vt::VT_COUNT],
    sw: vt::Switching,
}
pub fn with_grid<R>(f: impl FnOnce(&mut Console) -> R) -> Option<R> {
    static CONSOLE: std::sync::Mutex<Console> = std::sync::Mutex::new(Console {
        vts: [const { vt::Vt::new() }; vt::VT_COUNT],
        sw: vt::Switching::new(),
    });
    Some(f(&mut CONSOLE.lock().unwrap()))
}
pub fn wait_until_not_presenting() {}

#[test]
fn kd_changes_update_the_pending_frame_obligation() {
    let mut vts = [const { vt::Vt::new() }; vt::VT_COUNT];
    let mut sw = vt::Switching::new();
    let mut t = vt::Terminals {
        vts: &mut vts,
        sw: &mut sw,
    };
    let mut e = vt::Effects::default();
    t.set_mode(1, 9, vt::VT_PROCESS, 10, 12).unwrap();
    t.request(1, vt::Source::Ioctl, &mut e).unwrap();
    t.set_kd(1, 9, true, &mut e).unwrap();
    t.reldisp(1, 9, vt::VT_ACKACQ, &mut e).unwrap();
    assert!(t.active(1), "graphics no longer owes a text frame");

    t.request(0, vt::Source::Ioctl, &mut e).unwrap();
    t.reldisp(1, 9, 1, &mut e).unwrap();
    t.drawn(t.sw.epoch, &mut e);
    t.checked(0, &mut e);
    t.request(1, vt::Source::Ioctl, &mut e).unwrap();
    let obsolete_epoch = t.sw.epoch;
    t.set_kd(1, 9, false, &mut e).unwrap();
    t.reldisp(1, 9, vt::VT_ACKACQ, &mut e).unwrap();
    t.drawn(obsolete_epoch, &mut e);
    assert!(!t.active(1), "text must wait for a fresh frame");
    t.drawn(t.sw.epoch, &mut e);
    assert!(t.active(1));
}

#[test]
fn only_the_display_owner_can_consent_to_switching() {
    let mut vts = [const { vt::Vt::new() }; vt::VT_COUNT];
    let mut sw = vt::Switching::new();
    let mut t = vt::Terminals {
        vts: &mut vts,
        sw: &mut sw,
    };
    let mut e = vt::Effects::default();
    // An unrelated VT_PROCESS registration predates the display takeover.
    t.set_mode(0, 8, vt::VT_PROCESS, 10, 12).unwrap();
    t.take_display(9, 0, &mut e).unwrap();
    assert_eq!(t.request(1, vt::Source::Ioctl, &mut e), Err(16));
    assert_eq!(t.sw.fg, 0);
    t.set_mode(0, 8, vt::VT_AUTO, 0, 0).unwrap();
    assert_eq!(t.set_mode(0, 8, vt::VT_PROCESS, 10, 12), Err(16));
    t.set_mode(0, 9, vt::VT_PROCESS, 10, 12).unwrap();
    t.request(1, vt::Source::Ioctl, &mut e).unwrap();
    t.reldisp(0, 9, 0, &mut e).unwrap();
    assert_eq!(t.sw.fg, 0);
}

#[test]
fn late_ack_allows_release_before_readback() {
    let mut vts = [const { vt::Vt::new() }; vt::VT_COUNT];
    let mut sw = vt::Switching::new();
    let mut t = vt::Terminals {
        vts: &mut vts,
        sw: &mut sw,
    };
    let mut e = vt::Effects::default();
    t.set_mode(1, 9, vt::VT_PROCESS, 10, 12).unwrap();
    t.request(1, vt::Source::Ioctl, &mut e).unwrap();
    t.request(2, vt::Source::Ioctl, &mut e).unwrap();
    t.drawn(t.sw.epoch, &mut e);
    t.reldisp(1, 9, vt::VT_ACKACQ, &mut e).unwrap();
    assert!(t.active(1));
    // Contract 4.4: release starts now; acquisition waits for read-back.
    t.request(3, vt::Source::Ioctl, &mut e).unwrap();
    assert_eq!(t.sw.fg, 1);
    t.reldisp(1, 9, 1, &mut e).unwrap();
    assert_eq!(t.sw.fg, 1);
    t.checked(1, &mut e);
    assert_eq!(t.sw.fg, 3);
}

#[test]
fn latest_activation_replaces_the_request_queued_during_acquisition() {
    for release_before_readback in [false, true] {
        let mut vts = [const { vt::Vt::new() }; vt::VT_COUNT];
        let mut sw = vt::Switching::new();
        let mut t = vt::Terminals {
            vts: &mut vts,
            sw: &mut sw,
        };
        let mut e = vt::Effects::default();
        t.set_mode(1, 9, vt::VT_PROCESS, 10, 12).unwrap();
        t.request(1, vt::Source::Ioctl, &mut e).unwrap(); // tty2 acquiring
        t.request(2, vt::Source::Ioctl, &mut e).unwrap(); // tty3 queued
        t.drawn(t.sw.epoch, &mut e);
        t.reldisp(1, 9, vt::VT_ACKACQ, &mut e).unwrap();
        t.request(3, vt::Source::Ioctl, &mut e).unwrap(); // tty4 supersedes tty3
        if release_before_readback {
            t.reldisp(1, 9, 1, &mut e).unwrap();
        }
        assert_eq!(t.sw.fg, 1, "read-back still holds tty2");
        t.checked(1, &mut e);
        if !release_before_readback {
            t.reldisp(1, 9, 1, &mut e).unwrap();
        }
        assert_eq!(t.sw.fg, 3, "the latest destination is tty4");
        t.drawn(t.sw.epoch, &mut e);
        t.checked(3, &mut e);
        assert!(t.active(3), "tty3 must not replay after tty4's read-back");
    }
}

#[test]
fn preinstalled_history_survives_the_first_process_write() {
    let before = with_grid(|console| {
        let g = &mut console.vts[0].grid;
        g.set_storage(vec![grid::BLANK; 24].leak(), vec![grid::BLANK; 24].leak());
        g.set_size(8, 3);
        g.set_history(vec![grid::BLANK; grid::HISTORY_LINES * 8].leak());
        g.write(b"first\r\nsecond\r\nthird\r\nfourth");
        g.scroll_view(1);
        g.view_row(0).to_vec()
    })
    .unwrap();
    vt::ensure_history(0);
    with_grid(|console| {
        let g = &console.vts[0].grid;
        assert_eq!(g.view(), 1);
        assert_eq!(g.view_row(0), before);
    })
    .unwrap();
}

#[test]
fn cursor_presentations_follow_current_ownership_and_drain_before_text() {
    assert!(vt::Presentation::begin_cursor().is_none());
    vt::take_display(77, 1).unwrap();
    assert!(vt::Presentation::begin_cursor().is_none(), "takeover must quiesce first");
    syscall::graphics::DISPLAY_OWNER_GROUP.store(77, std::sync::atomic::Ordering::Release);
    syscall::graphics::DISPLAY_OWNER_PID.store(77, std::sync::atomic::Ordering::Release);
    let pass = vt::Presentation::begin_cursor().expect("foreground owner permits cursor");
    vt::give_back_display();
    assert!(vt::Presentation::begin_cursor().is_none());
    assert!(vt::wait_for_presentations(), "text must wait for the admitted cursor pass");
    drop(pass);
    assert!(!vt::wait_for_presentations());

    vt::take_display(77, 1).unwrap();
    with_grid(|console| {
        let mut t = vt::Terminals { vts: &mut console.vts, sw: &mut console.sw };
        let mut e = vt::Effects::default();
        t.set_mode(0, 77, vt::VT_PROCESS, 10, 12).unwrap();
        t.request(1, vt::Source::Ioctl, &mut e).unwrap();
        t.reldisp(0, 77, 1, &mut e).unwrap();
    });
    assert!(vt::Presentation::begin_cursor().is_none(), "background owner cannot draw cursor");
    assert!(vt::Presentation::begin(true).is_none(), "background compositor cannot present");
    assert!(vt::process_exited(77));
    assert!(vt::Presentation::begin_cursor().is_none(), "exited owner cannot draw cursor");
}
