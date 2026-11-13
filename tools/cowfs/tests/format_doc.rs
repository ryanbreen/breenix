use std::fs;

#[test]
fn format_doc_names_every_invariant_and_type() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/fs/cowfs-format.md");
    let doc = fs::read_to_string(path).expect("read normative cowfs format");
    for prefix in ["S", "N", "A", "C", "F", "W"] {
        let count = match prefix { "S" => 4, "N" | "A" => 6, "C" => 1, "F" => 10, "W" => 3, _ => unreachable!() };
        for n in 1..=count {
            let id = format!("**{prefix}{n}**");
            assert!(doc.contains(&id), "missing {id}");
        }
    }
    for token in ["INODE 0x01", "DIR_ENTRY 0x10", "DIR_INDEX 0x11", "EXTENT 0x20", "INLINE 0x21", "XATTR 0x30", "ORPHAN 0x40", "REF 0x80", "FREE 0x81", "PENDING 0x82", "DEAD 0x83", "ROOT 0x90", "0xE3069283", "0xa129ca6149be45e5", "exit 0", "exit 1", "exit 4", "exit 8"] {
        assert!(doc.contains(token), "missing {token}");
    }
}
