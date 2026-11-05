use std::{collections::BTreeSet, fs, path::Path};

fn scan(dir: &Path, root: &Path, found: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { scan(&path, root, found); continue; }
        if path.extension().is_none_or(|e| e != "rs") { continue; }
        let source = fs::read_to_string(&path).unwrap();
        if source.lines().filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains("input::report_key(") || l.contains("input::pointer::report(") || l.contains("hardware::report(") || l.contains("register_hardware(")) {
            found.insert(path.strip_prefix(root).unwrap().to_str().unwrap().to_owned());
        }
    }
}

#[test]
fn input_driver_callers_are_mapped_and_registration_is_shared() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let map = fs::read_to_string(root.join("docs/gui/input.md")).unwrap();
    let mut callers = BTreeSet::new();
    scan(&root.join("kernel/src"), root, &mut callers);
    for path in &callers {
        assert!(map.contains(path), "unmapped input caller: {path}");
    }
    for required in ["kernel/src/drivers/xhci/keyboard.rs", "kernel/src/drivers/xhci/pointer.rs", "kernel/src/drivers/virtio/input_mmio.rs"] {
        assert!(callers.contains(required), "missing hardware registration or reporting: {required}");
    }
    for row in ["xhci-hid-boot-keyboard", "xhci-hid-pointer", "virtio-input", "absolute device pending Q1"] {
        assert!(map.contains(row), "missing coverage row: {row}");
    }
    let hardware = fs::read_to_string(root.join("kernel/src/input/hardware.rs")).unwrap();
    assert!(!hardware.contains("target_arch"), "hardware registration must be architecture neutral");
    assert!(!hardware.contains("drivers::virtio"), "hardware registration must be driver neutral");
}
