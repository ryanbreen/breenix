#[path = "../../../kernel/src/graphics/boot_producer.rs"]
mod producer;
#[path = "../../../userspace/programs/src/suite_breengel/stage_serial.rs"]
mod serial;
#[test]
fn announcements_cannot_light_a_stage() {
    let marker = ["FILE_READ_TEST_PASSED|OTHER_PASSED"];
    for line in [
        "kernel::test_exec: FILE_READ_TEST_PASSED",
        "will emit FILE_READ_TEST_PASSED",
        "-> FILE_READ_TEST_PASSED",
        "FILE_READ_TEST_PASSED marker if successful",
    ] {
        assert_eq!(serial::reached(line, &marker), [false], "{line}");
        assert!(!producer::matches(line.as_bytes(), marker[0], &marker), "{line}");
    }
    assert_eq!(serial::reached("FILE_READ_TEST_PASSED\n", &marker), [true]);
    assert!(producer::matches(b"FILE_READ_TEST_PASSED", marker[0], &marker));
    assert_eq!(serial::reached("OTHER_PASSED\n", &marker), [true]);
    assert_eq!(
        serial::reached("FILE_READ_TEST_\nPASSED\n", &marker),
        [false]
    );
}
