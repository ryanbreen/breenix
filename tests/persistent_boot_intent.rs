//! Format 2 of the per-boot intent (kernel/src/boot/intent.rs): target, partitions and a test-binary image.
#[path = "../kernel/src/boot/intent.rs"]
mod intent;

#[test]
fn format_2_names_the_target_partitions_and_test_binaries() {
    let mut payload = b"BREENIX-BOOT 2\ntarget persistent\ncmdline gt710.clocks=post\nroot 2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe\n\
home 9bfeaef9-4f51-45de-8bcf-5cec1603b862\nuserland install-42\nprevious-userland install-41\ntests 4096 1024\n".to_vec();
    payload.resize(4096, 0);
    payload.extend_from_slice(b"BXTEST\0\0");
    assert!(intent::capture(&payload));
    assert_eq!(intent::get(), Some(&b"persistent\n"[..]));
    assert_eq!(intent::root_partition(), Some("2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe"));
    assert_eq!(intent::home_partition(), Some("9bfeaef9-4f51-45de-8bcf-5cec1603b862"));
    assert_eq!(intent::userland(), Some("install-42"));
    assert_eq!(intent::previous_userland(), Some("install-41"));
    for invalid in ["", ".", "..", "old.new", "old/new", "old\nnew", &"x".repeat(65)] {
        assert!(!intent::valid_generation(invalid));
    }
    assert_eq!(intent::tests(), Some((4096, 1024)));
    assert_eq!(intent::refused(), None);
    assert_eq!(intent::command_line(), Some("gt710.clocks=post"));
}

#[test]
fn test_boot_intent_accepts_the_arm_fixture_order() {
    let payload = b"BREENIX-BOOT 2\ntarget suite directories,filesystem,files-io\n\
root 085d88d8-0000-4000-a000-000000000001\n\
fixture 4096 67108864 read\nfixture 67112960 67108864 corrupt-node\n\
fixture 134221824 67108864 corrupt-data\nfixture 201330688 67108864 scratch\n";
    let captured = intent::parse(payload).unwrap();
    assert_eq!(intent::root_userland(Some(&captured)), Ok(None));
}
