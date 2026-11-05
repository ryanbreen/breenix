use std::{fs, path::Path};

#[test]
fn replay_control_uses_one_driver_interface_on_both_architectures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    let control = read("kernel/src/input/control.rs");
    assert!(!control.contains("target_arch"));
    assert!(!control.contains("virtio"));
    assert!(control.contains("trait Replay"));
    assert!(control.contains(".replay_keyboard("));
    assert!(control.contains(".replay_pointer("));
    for path in ["kernel/src/drivers/xhci/keyboard.rs", "kernel/src/drivers/virtio/input_mmio.rs"] {
        assert!(read(path).contains("impl crate::input::control::Replay"), "{} must implement replay", path);
    }
}
