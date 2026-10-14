#[path = "../libs/libbreenix-libc/src/pthread/cpuinfo.rs"]
mod cpuinfo;

#[test]
fn processor_records_survive_every_read_boundary() {
    let text = b"processor\t: 0\nFeatures: arbitrary\n\nprocessor : 1\nprocessor: 2";
    for boundary in 0..=text.len() {
        let mut count = cpuinfo::ProcessorCount::new();
        count.feed(&text[..boundary]);
        count.feed(&text[boundary..]);
        assert_eq!(count.get(), 3, "split at {boundary}");
    }
    let mut count = cpuinfo::ProcessorCount::new();
    for byte in text { count.feed(core::slice::from_ref(byte)); }
    assert_eq!(count.get(), 3);
}

#[test]
fn long_lines_and_similar_names_are_not_processors() {
    let mut count = cpuinfo::ProcessorCount::new();
    count.feed(b"processors: 1\nnotprocessor: 3\nprocessor model: cpu\n");
    count.feed(&[b'x'; 4096]);
    count.feed(b"\nprocessor\t: 63\n");
    assert_eq!(count.get(), 1);
    assert_eq!(cpuinfo::ProcessorCount::new().get(), 0);
}
