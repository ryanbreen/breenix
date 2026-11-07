//! Case-local SCR1 handoff to the parent's ordered evidence sender.
use super::evidence::{Route, Upload, MAX_TOTAL};
use std::{fs, io::{Read, Write}, path::Path};

pub(super) fn queue(path: &Path, known: &str, record: &screen_capture::Record<'_>) -> Result<bool, String> {
    let digest: String = record.digest.iter().map(|b| format!("{b:02x}")).collect();
    if known.lines().any(|line| line == digest) { return Ok(false); }
    let bytes = screen_capture::encode(record)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path).map_err(|_| "capture-queue-open")?;
    if file.metadata().map_err(|_| "capture-queue-size")?.len() + bytes.len() as u64 > MAX_TOTAL as u64 {
        return Err("capture-queue-bound".into());
    }
    file.write_all(&bytes).map_err(|_| "capture-queue-write")?;
    Ok(true)
}

pub(super) fn flush(path: &Path, upload: &mut Upload,
    mut send: impl FnMut(&Route, &str, &[u8], [u8; 32]) -> Result<(), String>) -> Vec<String> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => return vec!["SCREEN_UPLOAD refused=queue-open".into()],
    };
    let mut lines = Vec::new();
    // Read one record at a time and refuse an interrupted child's partial tail.
    loop {
        let mut header = [0; screen_capture::HEADER];
        match file.read(&mut header[..1]) {
            Ok(0) => break,
            Ok(1) => {},
            _ => { lines.push("SCREEN_UPLOAD refused=queue-read".into()); break; },
        }
        if file.read_exact(&mut header[1..]).is_err() {
            lines.push("SCREEN_UPLOAD refused=queue-partial".into()); break;
        }
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if !(512..=screen_capture::MAX_RECORD).contains(&length) || length % 512 != 0 {
            lines.push("SCREEN_UPLOAD refused=queue-length".into()); break;
        }
        let mut bytes = vec![0; length]; bytes[..header.len()].copy_from_slice(&header);
        if file.read_exact(&mut bytes[header.len()..]).is_err() {
            lines.push("SCREEN_UPLOAD refused=queue-partial".into()); break;
        }
        match screen_capture::decode(&bytes) {
            Ok((record, _)) => {
                let identity = format!("{} {} {}", record.suite, record.case, record.checkpoint);
                drop(record);
                lines.push(picture(upload, &identity, &bytes, &mut send));
            },
            Err(error) => { lines.push(format!("SCREEN_UPLOAD refused={error}")); break; },
        }
    }
    let _ = fs::remove_file(path);
    lines
}

pub(super) fn picture(upload: &mut Upload, identity: &str, data: &[u8],
    send: impl FnMut(&Route, &str, &[u8], [u8; 32]) -> Result<(), String>) -> String {
    match upload.send_parts(data, send) {
        Ok((first, parts, hash)) => format!("SCREEN_UPLOAD {identity} first={first} parts={parts} bytes={} sha256={hash}", data.len()),
        Err(error) => format!("SCREEN_UPLOAD {identity} refused={error}"),
    }
}
