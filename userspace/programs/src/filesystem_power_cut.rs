//! Record stream for the host's cold readback after a power cut.
use libbreenix::crypto::sha256::sha256;
use std::io::{self, Write};

const RECORD_BYTES: usize = 4096;
const DIGEST_OFFSET: usize = RECORD_BYTES - 32;

fn record(nonce: u64, seq: u64) -> ([u8; RECORD_BYTES], [u8; 32]) {
    let mut record = [0; RECORD_BYTES];
    record[..8].copy_from_slice(&seq.to_le_bytes());
    record[8..16].copy_from_slice(&nonce.to_le_bytes());
    for (index, word) in record[16..DIGEST_OFFSET].chunks_exact_mut(8).enumerate() {
        word.copy_from_slice(&(seq ^ index as u64).to_le_bytes());
    }
    let digest = sha256(&record[..DIGEST_OFFSET]);
    record[DIGEST_OFFSET..].copy_from_slice(&digest);
    (record, digest)
}

pub(super) fn write_records<W: Write>(
    log: &mut W,
    receipts: &mut impl Write,
    nonce: u64,
    mut sync: impl FnMut(&W) -> io::Result<()>,
    mut after_receipt: impl FnMut(&mut W, u64) -> io::Result<bool>,
) -> io::Result<()> {
    let mut seq = 1u64;
    loop {
        let (record, digest) = record(nonce, seq);
        log.write_all(&record)?;
        sync(log)?;

        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        // Start a fresh serial line even if the kernel left a breadcrumb unfinished.
        let line = format!("\nFSYNC {seq} {hex} nonce={nonce:016x}\n");
        receipts.write_all(line.as_bytes())?;
        receipts.flush()?;
        if after_receipt(log, seq)? {
            return Ok(());
        }
        seq += 1;
    }
}

pub(super) fn probe_record<W: Write>(
    log: &mut W,
    nonce: u64,
    seq: u64,
    sync: impl FnOnce(&W) -> io::Result<()>,
    read: impl FnOnce(&W, u64, &mut [u8]) -> io::Result<usize>,
) -> io::Result<()> {
    let (record, _) = record(nonce, seq);
    log.write_all(&record)?;
    sync(log)?;
    let mut bytes = [0; RECORD_BYTES];
    let offset = (seq - 1) * RECORD_BYTES as u64;
    match read(log, offset, &mut bytes) {
        Err(error) if error.raw_os_error() == Some(5) => Ok(()),
        Err(error) => Err(error),
        Ok(count) => Err(io::Error::other(format!("corruption probe returned {count} bytes"))),
    }
}

pub(super) fn diagnostic_done(serial: &mut impl Write, selection: &str) -> io::Result<()> {
    writeln!(serial, "\nSUITE filesystem DONE passed=1 failed=0 skipped=0 total=1 only={selection}")?;
    writeln!(serial, "SUITE_SEQUENCE READY filesystem")?;
    serial.flush()
}
