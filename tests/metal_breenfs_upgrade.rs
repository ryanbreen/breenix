#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/suite_disk.rs"]
mod suite_disk;
#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/persist.rs"]
mod persist;

#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/manager/clock.rs"]
mod clock;
#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/manager/lab.rs"]
mod lab;
#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/manager/options.rs"]
mod options;
#[allow(dead_code)]
#[path = "../scripts/metal/efi-stub/src/manager/slots.rs"]
mod slots;

#[test]
fn ext2_rescue_requires_both_live_partition_signatures() {
    let config = slots::parse_config(b"\
BREENIX-MANAGER 1\n\
query http://example.invalid/manager\n\
port 7fff0400\n\
rescue-partitions 33221100554477668899aabbccddeeff ffeeddccbbaa99887766554433221100\n").unwrap();
    let system = 0xFFEEDDCCBBAA99886677445500112233u128.to_le_bytes();
    let home = 0x00112233445566778899AABBCCDDEEFFu128.to_le_bytes();
    let breenfs = [7; 16];
    assert!(config.rescue_available([breenfs, home, system]));
    assert!(!config.rescue_available([breenfs, system]));
    assert!(!config.rescue_available([breenfs, home]));
    assert!(!config.rescue_available([breenfs]));
    assert!(matches!(lab::parse_answer(b"persistent attempt rescue"),
                     lab::Answer::Persistent(_, lab::Entry::Rescue, lab::SlotAction::None)));
    assert!(matches!(lab::parse_answer(b"persistent attempt rescue rollback"), lab::Answer::None(_)));
}
