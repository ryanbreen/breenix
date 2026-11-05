use std::{fs, path::Path};

fn compact(source: &str) -> String {
    source.lines().map(|line| line.split("//").next().unwrap())
        .collect::<String>().split_whitespace().collect()
}

#[test]
fn replay_control_uses_one_driver_interface_on_both_architectures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    let control = compact(&read("kernel/src/input/control.rs"));
    assert!(!control.contains("target_arch"));
    assert!(!control.contains("virtio"));
    assert!(!control.contains("drivers::"), "selectors must not name a driver");
    assert!(control.contains("traitReplay:Sync"));
    assert!(control.contains("1=>driver.replay_keyboard(suppress)"));
    assert!(control.contains("2=>driver.replay_pointer(x,y,suppress)"));
    assert!(control.contains("_=>Err(EINVALasu64)"));
    assert!(control.contains("authorize()?"));
    assert!(control.contains("test_control::enabled()"));
    for path in ["kernel/src/drivers/xhci/keyboard.rs", "kernel/src/drivers/virtio/input_mmio.rs"] {
        let driver = compact(&read(path));
        assert!(driver.contains("implcrate::input::control::Replay"), "{path} must implement replay");
        assert!(driver.contains("fnreplay_keyboard("));
        assert!(driver.contains("fnreplay_pointer("));
    }
    let fs = compact(&read("kernel/src/syscall/fs.rs"));
    assert!(fs.contains("input::control::open(normalized)"));
}
