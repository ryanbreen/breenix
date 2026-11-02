//! Verify the raw disk against the boot's complete serial SCREEN inventory.
use std::{collections::{HashMap,HashSet}, fs::File, io::{Read,Seek,SeekFrom}};
use screen_capture::{decode,DISK_MAGIC,HEADER,MAX_RECORD};
fn verify(disk: &str, serial: &str) -> Result<(),String> {
    let serial = std::fs::read_to_string(serial).map_err(|e| e.to_string())?;
    let mut first = HashMap::new();
    let mut keys = HashSet::new();
    for line in serial.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.len() < 3 || words[0] != "SUITE" || words[2] != "SCREEN" { continue; }
        if words.len() < 7 || !words[5].starts_with("sha256=") {
            return Err(format!("non-picture SCREEN record: {line}"));
        }
        if !keys.insert((words[1].to_string(),words[3].to_string(),words[4].to_string())) {
            return Err(format!("duplicate SCREEN record: {line}"));
        }
        let hash = words[5].strip_prefix("sha256=").unwrap();
        let mode = words[6].strip_prefix("mode=").ok_or("serial mode")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("serial digest".into()); }
        first.entry(hash.to_string()).or_insert_with(||
            (words[1].to_string(),words[3].to_string(),words[4].to_string(),mode.to_string()));
    }
    if first.is_empty() { return Err("serial has no SCREEN pictures".into()); }
    let mut input = File::open(disk).map_err(|e| e.to_string())?;
    let mut marker = [0;512]; input.read_exact(&mut marker).map_err(|e| e.to_string())?;
    if &marker[..8] != DISK_MAGIC || marker[8..].iter().any(|&b| b != 0) { return Err("disk marker".into()); }
    let mut seen = HashSet::new();
    let mut offset = 512u64;
    let mut raw = 0; let mut rle = 0;
    loop {
        let mut header = [0;HEADER]; input.read_exact(&mut header).map_err(|e| e.to_string())?;
        if header == [0;HEADER] {
            let mut tail = [0;65536];
            loop { let n=input.read(&mut tail).map_err(|e| e.to_string())?;
                if n == 0 { break; } if tail[..n].iter().any(|&b| b != 0) { return Err("data after end".into()); }
            }
            break;
        }
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if !(HEADER..=MAX_RECORD).contains(&length) { return Err("record length".into()); }
        let mut bytes = vec![0;length]; bytes[..HEADER].copy_from_slice(&header);
        input.read_exact(&mut bytes[HEADER..]).map_err(|e| e.to_string())?;
        let (record,_) = decode(&bytes).map_err(String::from)?;
        let hash: String = record.digest.iter().map(|b| format!("{b:02x}")).collect();
        let expected = first.get(&hash).ok_or_else(|| format!("disk digest absent from serial: {hash}"))?;
        let identity = (record.suite,record.case,record.checkpoint,format!("{}x{}",record.width,record.height));
        if &identity != expected { return Err(format!("first occurrence mismatch: {identity:?} != {expected:?}")); }
        if !seen.insert(hash) { return Err("duplicate disk digest".into()); }
        if bytes[8] == 0 { raw += 1; } else { rle += 1; }
        offset += length as u64;
        input.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    }
    if seen.len() != first.len() { return Err(format!("missing disk pictures: serial={} disk={}",first.len(),seen.len())); }
    println!("CAPTURE PASS serial_checkpoints={} distinct={} records={} raw={} rle={} bytes={}",
        keys.len(),first.len(),seen.len(),raw,rle,offset);
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 { eprintln!("usage: verify CAPTURE_DISK SERIAL"); std::process::exit(2); }
    if let Err(e) = verify(&args[1],&args[2]) { eprintln!("CAPTURE FAIL: {e}"); std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_verification_requires_serial_identity_and_complete_coverage() {
        let dir = std::env::temp_dir().join(format!("scr1-verify-{}",std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let disk = dir.join("disk"); let serial = dir.join("serial");
        let pixels = [1,2,3,255]; let digest = libpixcheck::sha256(&pixels);
        let bytes = screen_capture::encode(&screen_capture::Record { suite:"gpu",case:"draw/one",
            checkpoint:"end",width:1,height:1,digest,pixels:&pixels }).unwrap();
        let mut data = vec![0;4096]; data[..8].copy_from_slice(DISK_MAGIC);
        data[512..512+bytes.len()].copy_from_slice(&bytes); std::fs::write(&disk,&data).unwrap();
        let hash: String=digest.iter().map(|b|format!("{b:02x}")).collect();
        let line=format!("SUITE gpu SCREEN draw/one end sha256={hash} mode=1x1 binding=1x1\n");
        std::fs::write(&serial,&line).unwrap();
        assert!(verify(disk.to_str().unwrap(),serial.to_str().unwrap()).is_ok());
        for bad in [line.replace("mode=1x1","mode=2x1"),line.replace("draw/one","draw/two"),
            format!("{line}SUITE gpu SCREEN draw/two end sha256={} mode=1x1\n","0".repeat(64))] {
            std::fs::write(&serial,bad).unwrap();
            assert!(verify(disk.to_str().unwrap(),serial.to_str().unwrap()).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
