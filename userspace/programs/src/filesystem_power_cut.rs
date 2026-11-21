//! Record stream for the host's cold readback after a power cut.
use libbreenix::crypto::sha256::sha256;
use std::io::{self, Write};

const RECORD_BYTES: usize = 4096;
const DIGEST_OFFSET: usize = RECORD_BYTES - 32;

pub(super) fn write_records<W: Write>(
    log: &mut W,
    receipts: &mut impl Write,
    nonce: u64,
    mut sync: impl FnMut(&W) -> io::Result<()>,
    mut after_receipt: impl FnMut(&mut W, u64) -> io::Result<()>,
) -> io::Result<()> {
    let mut record = [0; RECORD_BYTES];
    record[8..16].copy_from_slice(&nonce.to_le_bytes());
    let mut seq = 1u64;
    loop {
        record[..8].copy_from_slice(&seq.to_le_bytes());
        for (index, word) in record[16..DIGEST_OFFSET].chunks_exact_mut(8).enumerate() {
            word.copy_from_slice(&(seq ^ index as u64).to_le_bytes());
        }
        let digest = sha256(&record[..DIGEST_OFFSET]);
        record[DIGEST_OFFSET..].copy_from_slice(&digest);

        log.write_all(&record)?;
        sync(log)?;

        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        // Start a fresh serial line even if the kernel left a breadcrumb unfinished.
        let line = format!("\nFSYNC {nonce:016x} {seq} {hex}\n");
        receipts.write_all(line.as_bytes())?;
        receipts.flush()?;
        after_receipt(log, seq)?;
        seq += 1;
    }
}
