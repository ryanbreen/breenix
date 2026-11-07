#[path = "../libs/libuitest/src/arena.rs"]
pub mod arena;
#[path = "../libs/libuitest/src/artifacts.rs"]
pub mod artifacts;
use arena::{Arena, Layout, Slot, SlotRead, Record};
use std::{fs, path::PathBuf};
fn root(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("uitest-arena-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir(&path).unwrap(); path
}
#[test]
fn slots_follow_current_mode_and_refuse_outside_scr1_limit() {
    for (w, h) in [(1280, 800), (1280, 1024), (2560, 1600)] {
        let layout = Layout::for_mode(w, h).unwrap();
        assert_eq!(layout.capacity(Slot::Screen), w as usize * h as usize * 4);
        assert_eq!(layout.capacity(Slot::Frame), w as usize * h as usize * 4);
        assert_eq!(layout.capacity(Slot::Tree), 1024 * 1024);
    }
    for (w, h) in [(0, 1), (1, 0), (2561, 1600), (2560, 1601), (u32::MAX, u32::MAX)] {
        assert!(Layout::for_mode(w, h).is_err());
    }
}
#[test]
fn writing_is_partial_and_release_publishes_record_and_bytes_once() {
    let root = root("publication"); let path = root.join("arena");
    let mut writer = Arena::create(&path, Layout::for_mode(8, 4).unwrap()).unwrap();
    let reader = Arena::open(&path).unwrap();
    assert!(matches!(reader.read(Slot::Screen).unwrap(), SlotRead::Free));
    let mut slot = writer.begin(Slot::Screen).unwrap();
    slot.bytes_mut()[..128].fill(0x3d);
    assert!(matches!(reader.read(Slot::Screen).unwrap(), SlotRead::Partial("case-died")));
    let record = Record { check: "planted".into(), window: 17, commit: 41, epoch: 3,
        composite: 12, width: 8, height: 4, ..Record::default() };
    slot.publish(record, 128).unwrap();
    let SlotRead::Final { record, bytes } = reader.read(Slot::Screen).unwrap() else { panic!("final not published") };
    assert_eq!((record.check.as_str(), record.window, record.commit, record.epoch, record.composite), ("planted", 17, 41, 3, 12));
    assert_eq!(bytes, vec![0x3d; 128]);
    assert!(writer.begin(Slot::Screen).is_err());
    let mut tree = writer.begin(Slot::Tree).unwrap(); tree.bytes_mut()[0] = 7; drop(tree);
    assert!(matches!(reader.read(Slot::Tree).unwrap(), SlotRead::Partial("case-died")));
    // Altering this committed payload must produce a hash error.
    use std::os::unix::fs::FileExt;
    fs::OpenOptions::new().write(true).open(&path).unwrap().write_at(&[0], writer.layout().data_offset(Slot::Screen) as u64).unwrap();
    assert!(reader.read(Slot::Screen).unwrap_err().contains("hash"));
    drop(writer); drop(reader); fs::remove_dir_all(root).unwrap();
}
#[test]
fn artifact_publication_is_local_complete_and_replaces_no_partial_file() {
    let root = root("artifacts");
    let mut store = artifacts::Store::new(&root, "ui-testing", "runner/death").unwrap();
    store.write("input.log", b"serial=7 delivered\n").unwrap();
    store.write("stderr-17.log", b"last app bytes").unwrap();
    store.finish("trigger=crash\nframe=partial:case-died\n").unwrap();
    let path = root.join("ui-testing/runner/death");
    assert_eq!(fs::read(path.join("input.log")).unwrap(), b"serial=7 delivered\n");
    let manifest = fs::read_to_string(path.join("collection.txt")).unwrap();
    assert!(manifest.contains("trigger=crash") && manifest.contains("input.log") && manifest.contains("stderr-17.log"));
    assert!(fs::read_dir(path).unwrap().all(|p| !p.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    assert!(artifacts::Store::new(&root, "../escape", "runner/death").is_err());
    assert!(store.write("../escape", b"x").is_err());
    fs::remove_dir_all(root).unwrap();
}
