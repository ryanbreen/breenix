//! Format 2 of the per-boot intent (kernel/src/boot/intent.rs): target, partitions and a test-binary image.
#[path = "../kernel/src/boot/intent.rs"]
mod intent;

#[test]
fn format_2_names_the_target_partitions_and_test_binaries() {
    let mut payload = b"BREENIX-BOOT 2\ntarget persistent\nroot 2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe\n\
home 9bfeaef9-4f51-45de-8bcf-5cec1603b862\nuserland install-42\ntests 4096 1024\n".to_vec();
    payload.resize(4096, 0);
    payload.extend_from_slice(b"BXTEST\0\0");
    assert!(intent::capture(&payload));
    assert_eq!(intent::get(), Some(&b"persistent\n"[..]));
    assert_eq!(intent::root_partition(), Some("2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe"));
    assert_eq!(intent::home_partition(), Some("9bfeaef9-4f51-45de-8bcf-5cec1603b862"));
    assert_eq!(intent::userland(), Some("install-42"));
    for invalid in ["", ".", "..", "old.new", "old/new", "old\nnew", &"x".repeat(65)] {
        assert!(!intent::valid_generation(invalid));
    }
    assert_eq!(intent::tests(), Some((4096, 1024)));
    assert_eq!(intent::refused(), None);
}
