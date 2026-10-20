//! A format-2 intent that does not parse is kept as a refusal, not a RAM-disk root (kernel/src/boot/intent.rs).
// home_partition and tests read a parsed intent's items, which a refused intent lacks.
#[allow(dead_code)]
#[path = "../kernel/src/boot/intent.rs"]
mod intent;

#[test]
fn a_damaged_format_2_intent_is_refused_and_names_nothing() {
    // The root is not a GUID.
    assert!(intent::capture(b"BREENIX-BOOT 2\ntarget default\nroot not-a-guid\n"));
    assert_eq!(intent::get(), None);
    assert_eq!(intent::root_partition(), None);
    assert!(intent::refused().is_some());
}
