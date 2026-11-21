extern crate self as libbreenix;

#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;

pub mod crypto {
    pub use crate::sha256;
}

#[path = "../userspace/programs/src/filesystem_power_cut.rs"]
mod writer;

use std::io;

#[test]
fn power_cut_writer_acknowledges_only_complete_synced_records() {
    const RECORD_BYTES: usize = 4096;
    const DIGESTS: [&str; 2] = [
        "9d341e061c7d4d67541c8c5d575d1fb471ab3687128d048a7d50123d5ce110e7",
        "5630ad31aa545bff13439ebfbc7ff061b13fd2a887d0d9996947780a0e09e39a",
    ];
    let nonce = 0x0123456789abcdef_u64;
    let mut log = Vec::new();
    let mut receipts = Vec::new();
    let mut sync_calls = 0;
    let result = writer::write_records(&mut log, &mut receipts, nonce, |log| {
        sync_calls += 1;
        assert_eq!(log.len(), sync_calls * RECORD_BYTES);
        if sync_calls == 3 {
            return Err(io::Error::other("injected fsync failure"));
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err().to_string(), "injected fsync failure");
    assert_eq!(sync_calls, 3);

    for (index, record) in log.chunks_exact(RECORD_BYTES).enumerate() {
        assert_eq!(&record[..8], &(index as u64 + 1).to_le_bytes());
        assert_eq!(&record[8..16], &nonce.to_le_bytes());
        let (body, digest) = record.split_at(RECORD_BYTES - 32);
        assert_eq!(digest, sha256::sha256(body));
        if let Some(expected) = DIGESTS.get(index) {
            assert_eq!(hex(digest), *expected);
        }
    }
    assert_eq!(
        String::from_utf8(receipts).unwrap(),
        format!("\nFSYNC {nonce:016x} 1 {}\n\nFSYNC {nonce:016x} 2 {}\n", DIGESTS[0], DIGESTS[1]),
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
