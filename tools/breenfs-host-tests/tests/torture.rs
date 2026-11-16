use std::{fs, process::Command};

#[test]
fn short_run_writes_a_summary() {
    let dir = std::env::temp_dir().join(format!("breenfs-torture-test-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_breenfs-torture"))
        .args(["--hours", "0.002", "--checker", env!("CARGO_BIN_FILE_BREENFS_breenfs-check"), "--out"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let summary: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("summary.json")).unwrap()).unwrap();
    assert!(summary["images_checked"].as_u64().unwrap() > 0, "{summary}");
    assert!(summary["first_seed"].as_u64().is_some(), "{summary}");
    assert!(summary["last_seed"].as_u64().unwrap() >= summary["first_seed"].as_u64().unwrap(), "{summary}");
    assert_eq!(summary["failures"].as_u64(), Some(0), "{summary}");
    fs::remove_dir_all(dir).unwrap();
}


#[path = "../src/test_image.rs"]
mod test_image;

#[test]
fn torture_stream_reuses_slots_and_splits_trees() {
    use breenfs_host_tests::{
        btree::NodeStore, commit::DeviceStore, format::{NodeHeader, Root, Superblock},
        model::Model, recording::RecordingDevice, stream::{Stream, torture_operations}, Fs, WriterAdapter,
    };
    use std::sync::Arc;
    test_image::check(&test_image::image(), "torture-source");
    assert_ne!(torture_operations(1), torture_operations(2));
    for seed in [1, 2] {
        let length = torture_operations(seed);
        assert!(length >= 32, "seed {seed}: {length} operations");
        let device = Arc::new(RecordingDevice::new(test_image::image()).unwrap());
        let mut writer = WriterAdapter::new(device.clone()).unwrap();
        let mut model = Model::new();
        let mut generation = 0;
        let mut height = 0;
        for op in Stream::new(seed).take(length) {
            assert_eq!(writer.execute(&op), model.execute(&op), "{op:?}");
            assert_eq!(writer.state().unwrap(), model.state().unwrap(), "{op:?}");
            let bytes = device.durable_image();
            for slot in 16..20 {
                if let Ok(sb) = Superblock::decode(&bytes[slot*4096..(slot+1)*4096]) {
                    generation = generation.max(sb.generation);
                    let store = DeviceStore(device.as_ref());
                    let h = NodeHeader::decode(&store.get(sb.space_root.addr).unwrap()).unwrap();
                    height = height.max(h.level + 1);
                    let value = breenfs_host_tests::btree::search(&store, sb.space_root,
                        breenfs_host_tests::format::Key { object: 0xffff_ffff_ffff_ff10, item_type: breenfs_host_tests::format::ROOT, offset: 1 }).unwrap().unwrap();
                    height = height.max(Root::decode(&value).unwrap().root.level + 1);
                }
            }
        }
        assert!(generation >= 9, "seed {seed}: generation {generation}");
        assert!(height >= 2, "seed {seed}: height {height}");
    }
}
