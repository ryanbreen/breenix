#[path = "../src/test_image.rs"]
mod test_image;

#[test]
fn torture_stream_reuses_slots_and_splits_trees() {
    use breenfs_host_tests::{
        btree::NodeStore,
        commit::DeviceStore,
        format::{NodeHeader, Root, Superblock},
        model::Model,
        recording::RecordingDevice,
        stream::{torture_operations, torture_seed, Stream},
        Fs, WriterAdapter,
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
        for op in Stream::new(torture_seed(seed)).take(length) {
            assert_eq!(writer.execute(&op), model.execute(&op), "{op:?}");
            assert_eq!(writer.state().unwrap(), model.state().unwrap(), "{op:?}");
            let bytes = device.durable_image();
            let slots: [Option<Superblock>; 4] = core::array::from_fn(|index| {
                let slot = 16 + index;
                Superblock::decode(&bytes[slot * 4096..(slot + 1) * 4096]).ok()
            });
            let max_gen = slots.iter().flatten().map(|sb| sb.generation).max().unwrap();
            for slot in 16..20 {
                if let Some(sb) = &slots[slot - 16] {
                    generation = generation.max(sb.generation);
                    if sb.generation < max_gen.saturating_sub(1) {
                        continue;
                    }
                    let store = DeviceStore(device.as_ref());
                    let h = NodeHeader::decode(&store.get(sb.space_root.addr).unwrap()).unwrap();
                    height = height.max(h.level + 1);
                    let value = breenfs_host_tests::btree::search(
                        &store,
                        sb.space_root,
                        breenfs_host_tests::format::Key {
                            object: 0xffff_ffff_ffff_ff10,
                            item_type: breenfs_host_tests::format::ROOT,
                            offset: 1,
                        },
                    )
                    .unwrap()
                    .unwrap();
                    height = height.max(Root::decode(&value).unwrap().root.level + 1);
                }
            }
        }
        assert!(generation >= 9, "seed {seed}: generation {generation}");
        assert!(height >= 2, "seed {seed}: height {height}");
    }
}
