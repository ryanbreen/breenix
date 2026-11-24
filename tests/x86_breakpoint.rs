#[allow(dead_code)]
#[path = "../kernel/src/signal/constants.rs"]
mod signal_constants;
mod signal {
    pub(crate) use super::signal_constants as constants;
}

#[path = "../kernel/src/arch_impl/x86_64/breakpoint.rs"]
mod breakpoint;

use breakpoint::{from_userspace, PendingBreakpoint};

#[test]
fn breakpoint_handoff_uses_cpl3_and_reports_the_int3_address() {
    assert!(!from_userspace(0x08));
    assert!(!from_userspace(0x09));
    assert!(!from_userspace(0x0a));
    assert!(from_userspace(0x23));

    let pending = PendingBreakpoint::default();
    assert!(!pending.is_pending());
    pending.record(0x401001);
    assert!(pending.is_pending());
    assert_eq!(pending.take(), Some((5, 1, 0x401000)));
    assert!(!pending.is_pending());
    assert_eq!(pending.take(), None);
}
