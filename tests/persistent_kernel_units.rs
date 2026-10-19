//! The persistent install's pure kernel pieces without a guest (docs/metal/README.md, "Boot intents and root
//! selection" and "The write fence"): the GUID partition table reader and the write fence's ranges. Their own unit
//! tests run here.
// The table's sector counts and disk GUID serve the kernel's root selection, which needs a disk.
#[allow(dead_code)]
#[path = "../kernel/src/block/gpt.rs"]
mod gpt;
// The policy's entry points are the drivers'; on this host only the fence's ranges compile and are tested.
#[allow(dead_code)]
#[path = "../kernel/src/block/write_policy.rs"]
mod write_policy;

#[test]
fn the_layout_type_guids_match_the_tools() {
    // scripts/metal/metal_persist.py writes these in text form; the kernel checks a partition's role by them.
    assert_eq!(gpt::parse_guid("dac951c3-27df-4e3d-8b9e-487906ac2c81"), Some(gpt::TEST_ROOT_TYPE));
    assert_eq!(gpt::parse_guid("2ef14a34-abbd-48f8-83d6-f2a8c7b4a9fe"), Some(gpt::SYSTEM_TYPE));
    assert_eq!(gpt::parse_guid("9bfeaef9-4f51-45de-8bcf-5cec1603b862"), Some(gpt::HOME_TYPE));
}
