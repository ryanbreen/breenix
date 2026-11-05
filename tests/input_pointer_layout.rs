//! Shared HID capabilities and the retained syscall-413 pointer snapshot.
#[path = "../kernel/src/drivers/hid_boot/pointer.rs"]
pub mod hid_pointer;
mod drivers {
    pub mod hid_boot {
        pub use crate::hid_pointer as pointer;
    }
}
#[path = "../kernel/src/input/pointer.rs"]
pub mod pointer;

#[test]
fn pointer_capabilities_follow_the_report_descriptor() {
    // Relative X/Y and wheel, with three buttons (HID 1.11 B.2 plus wheel).
    let descriptor = [
        0x05, 1, 0x09, 2, 0xa1, 1, 0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1, 0x95, 3, 0x75, 1,
        0x81, 2, 0x95, 1, 0x75, 5, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15,
        0x81, 0x25, 0x7f, 0x75, 8, 0x95, 3, 0x81, 6, 0xc0,
    ];
    let layout = hid_pointer::Layout::parse(&descriptor).unwrap();
    let mut caps = Vec::new();
    layout.capabilities(&mut |kind, code, min, max| caps.push((kind, code, min, max)));
    assert_eq!(
        caps,
        [
            (1, 272, 0, 1),
            (1, 273, 0, 1),
            (1, 274, 0, 1),
            (2, 0, -127, 127),
            (2, 1, -127, 127),
            (2, 8, -127, 127)
        ]
    );
    pointer::set_screen(100, 80);
    pointer::move_to(98, 1);
    pointer::report(2, layout.decode(&[5, 10, 0xfb, 0xff]).unwrap().unwrap());
    assert_eq!(pointer::mouse_state(), (99, 0, 5));
    assert_eq!(pointer::mouse_wheel_consume(), -1);
    pointer::release(2);
    assert_eq!(pointer::mouse_state(), (99, 0, 0));
    // The same descriptor with absolute axes advertises their logical range.
    let mut absolute = descriptor;
    let last = absolute.len() - 2;
    absolute[last] = 2;
    let mut caps = Vec::new();
    hid_pointer::Layout::parse(&absolute)
        .unwrap()
        .capabilities(&mut |kind, code, min, max| caps.push((kind, code, min, max)));
    assert_eq!(caps[3..5], [(3, 0, -127, 127), (3, 1, -127, 127)]);
}
