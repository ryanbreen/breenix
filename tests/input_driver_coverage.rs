use std::{fs, path::Path};

#[test]
fn every_production_input_reporter_has_a_target_row() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let map = fs::read_to_string(root.join("docs/gui/input.md")).unwrap();
    let reporters = [
        ("kernel/src/keyboard.rs", "crate::input::report_key(", "i8042 PS/2"),
        ("kernel/src/drivers/hid_boot/mod.rs", "crate::input::report_key(", "xHCI USB HID boot keyboard"),
        ("kernel/src/drivers/xhci/pointer.rs", "crate::input::pointer::report(", "xHCI HID tablet"),
        ("kernel/src/drivers/virtio/input_mmio.rs", "hardware::report(", "virtio-input MMIO"),
    ];
    let expected = reporters.iter().map(|(path, _, _)| *path).collect::<Vec<_>>();
    let patterns = reporters.iter().map(|(_, call, _)| *call).collect::<Vec<_>>();
    fn scan(dir: &Path, root: &Path, expected: &[&str], patterns: &[&str]) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { scan(&path, root, expected, patterns); continue; }
            if path.extension().is_none_or(|e| e != "rs") { continue; }
            // Clean-room boundary: these renderer implementations are not read.
            if ["gpu_pci.rs", "virgl.rs", "svga3.rs"].iter().any(|name| path.file_name().unwrap() == *name) { continue; }
            let source = fs::read_to_string(&path).unwrap();
            if source.lines().filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| patterns.iter().any(|call| line.contains(call))) {
                let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
                assert!(expected.contains(&relative), "unmapped production input reporter: {relative}");
            }
        }
    }
    for (path, call, row) in reporters {
        let source = fs::read_to_string(root.join(path)).unwrap();
        assert!(source.lines().filter(|line| !line.trim_start().starts_with("//")).any(|line| line.contains(call)), "report path disappeared: {path}");
        assert!(map.contains(row), "target map does not name {row}");
    }
    scan(&root.join("kernel/src"), root, &expected, &patterns);
}
