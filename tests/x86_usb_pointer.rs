//! x86 HID pointer delivery and its xHCI worker wiring.
#![cfg(target_arch = "x86_64")]
#[path = "../kernel/src/drivers/hid_boot/pointer.rs"]
pub mod hid_pointer;
mod drivers { pub mod hid_boot { pub use crate::hid_pointer as pointer; } }
#[path = "../kernel/src/input/pointer.rs"]
mod pointer;

#[test]
fn x86_usb_pointer_report_reaches_screen_state() {
    // HID 1.11 boot mouse layout with a wheel.
    let descriptor = [
        0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 1, 0xa1, 0, 0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1,
        0x95, 3, 0x75, 1, 0x81, 2, 0x95, 1, 0x75, 5, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 3, 0x81, 6, 0xc0, 0xc0,
    ];
    pointer::set_screen(100, 80);
    pointer::move_to(98, 1);
    let motion = hid_pointer::Layout::parse(&descriptor).unwrap().decode(&[5, 10, 0xfb, 0xff]).unwrap().unwrap();
    pointer::report(1, motion);
    assert_eq!(pointer::mouse_state(), (99, 0, 5));
    assert_eq!(pointer::mouse_wheel_consume(), -1);

    // Ensure x86 compiles the worker path that calls this decoder and state.
    let module = include_str!("../kernel/src/drivers/xhci/mod.rs");
    let worker = include_str!("../kernel/src/drivers/xhci/keyboard.rs");
    for source in [module, worker] {
        for line in source.lines().collect::<Vec<_>>().windows(2) {
            if line[1].contains("mod pointer;") || line[1].contains("pub use pointer::describe")
                || line[1].contains("pointers: Vec<pointer::Pipe>") || line[1].contains("pointers: Vec::new()")
                || line[1].contains("if self.dispatch_pointer(event)") || line[1].contains("if let Some(index) = self.devices.iter().position(|d| d.port == port)")
                || line[1].contains("if !device.pointers.is_empty()") || line[1].contains("dcis.extend(device.pointers")
                || line[1].contains("for pipe in device.pointers") {
                assert_ne!(line[0].trim(), "#[cfg(target_arch = \"aarch64\")]", "x86 pointer path disabled: {}", line[1]);
            }
        }
    }
}
