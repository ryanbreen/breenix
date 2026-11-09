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


#[test]
fn tree_at_compared_commit_survives_thirty_updates_and_case_death_before_saving() {
    let root = root("historical");
    let path = root.join("arena");
    let mut case = Arena::create(&path, Layout::for_mode(2, 1).unwrap()).unwrap();
    let compared = Record { check: "label".into(), window: 17, commit: 41,
        epoch: 3, width: 2, height: 1, ..Record::default() };
    case.write_frame_tree(|pixels, tree| {
        pixels[..8].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        tree[..12].copy_from_slice(b"commit=41\0\0\0");
        Ok((compared.clone(), 8))
    }).unwrap();
    let mut screen = case.begin(Slot::Screen).unwrap();
    screen.bytes_mut()[..8].fill(19);
    screen.publish(Record { check: "label".into(), composite: 12, width: 2, height: 1,
        observation: vec![7; 1024], ..Record::default() }, 8).unwrap();
    // The live producer reuses its own storage after the compared capture.
    let mut live_tree = b"commit=41".to_vec();
    for commit in 42..=71 { live_tree = format!("commit={commit}").into_bytes(); }
    assert_eq!(live_tree, b"commit=71");
    drop(case); // No artifact write ran in the case.
    let collector = Arena::open(&path).unwrap();
    let SlotRead::Final { record, bytes } = collector.read(Slot::Tree).unwrap() else { panic!("lost tree") };
    assert_eq!((record.check.as_str(), record.window, record.commit, record.epoch), ("label", 17, 41, 3));
    assert_eq!(&bytes[..10], b"commit=41\0");
    let SlotRead::Final { record, bytes } = collector.read(Slot::Frame).unwrap() else { panic!("lost frame") };
    assert_eq!(record.commit, 41);
    assert_eq!(bytes, [1, 2, 3, 255, 4, 5, 6, 255]);
    let SlotRead::Final { record, bytes } = collector.read(Slot::Screen).unwrap() else { panic!("lost screen") };
    assert_eq!(record.composite, 12);
    assert_eq!(record.observation, vec![7; 1024]);
    assert_eq!(bytes, vec![19; 8]);
    drop(collector);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_screen_retained_and_interrupted_frame_tree_publication_is_partial() {
    let root = root("interrupted-pair");
    let path = root.join("arena");
    let mut case = Arena::create(&path, Layout::for_mode(2, 1).unwrap()).unwrap();
    let mut screen = case.begin(Slot::Screen).unwrap();
    screen.bytes_mut()[..8].fill(29);
    screen.publish(Record { check: "failed".into(), composite: 9, width: 2, height: 1,
        observation: vec![3; 1024], ..Record::default() }, 8).unwrap();
    // Force the exact interval between write_frame_tree's two release stores.
    let mut frame = case.begin(Slot::Frame).unwrap();
    frame.bytes_mut()[..8].fill(31);
    drop(frame);
    let mut tree = case.begin(Slot::Tree).unwrap();
    tree.bytes_mut()[..10].copy_from_slice(b"commit=41\0");
    tree.publish(Record { check: "failed".into(), window: 17, commit: 41,
        width: 2, height: 1, ..Record::default() }, 10).unwrap();
    drop(case);
    let collector = Arena::open(&path).unwrap();
    assert!(matches!(collector.read(Slot::Frame).unwrap(), SlotRead::Partial("case-died")));
    assert!(matches!(collector.read(Slot::Tree).unwrap(), SlotRead::Partial("case-died")),
        "a tree is not a completed capture until its paired frame publishes");
    let SlotRead::Final { record, bytes } = collector.read(Slot::Screen).unwrap() else { panic!("failure screen lost") };
    assert_eq!(record.composite, 9);
    assert_eq!(bytes, vec![29; 8]);
    drop(collector);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn checkpoints_replace_the_active_path_without_mutating_older_readers() {
    let path = std::env::temp_dir().join(format!("ui-checkpoint-{}", std::process::id()));
    let mut active = arena::Arena::create(&path, arena::Layout::for_mode(2, 1).unwrap()).unwrap();
    let mut writer = active.begin(arena::Slot::Frame).unwrap();
    writer.bytes_mut()[..4].copy_from_slice(&[1, 2, 3, 255]);
    writer.publish(arena::Record { check: "first".into(), ..Default::default() }, 4).unwrap();
    let older = arena::Arena::open(&path).unwrap();
    active.next_checkpoint().unwrap();
    let mut writer = active.begin(arena::Slot::Frame).unwrap();
    writer.bytes_mut()[..4].copy_from_slice(&[4, 5, 6, 255]);
    writer.publish(arena::Record { check: "second".into(), ..Default::default() }, 4).unwrap();
    let newest = older.reopen().unwrap();
    for (view, check, expected) in [(&older, "first", [1,2,3,255]), (&newest, "second", [4,5,6,255])] {
        match view.read(arena::Slot::Frame).unwrap() {
            arena::SlotRead::Final {record, bytes} => { assert_eq!(record.check, check); assert_eq!(bytes, expected); },
            other => panic!("incomplete checkpoint {other:?}"),
        }
    }
    drop((active, older, newest));
    std::fs::remove_file(path).unwrap();
}
