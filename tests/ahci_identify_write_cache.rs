#[path = "../kernel/src/drivers/ahci/identify.rs"]
mod identify;

#[test]
fn identify_reports_support_and_enabled_bits_independently() {
    for (word82, word85, expected) in [
        (0x0048u16, 0x2048u16, (false, false)),
        (0x0068, 0x2048, (true, false)),
        (0x0068, 0x2068, (true, true)),
        (0x0048, 0x2068, (false, true)),
    ] {
        let mut identify_sector = [0; 512];
        identify_sector[164..166].copy_from_slice(&word82.to_le_bytes());
        identify_sector[170..172].copy_from_slice(&word85.to_le_bytes());
        assert_eq!(identify::write_cache(&identify_sector), expected);
    }
}
