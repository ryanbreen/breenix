//! Send actual compressed readback rows to the boot service; serial keeps their digest.
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

const MAX_UPLOAD: usize = 2 << 20;
// Retained canonical payload total, including startup, pictures and final evidence.
const MAX_TOTAL: usize = 64 << 20;
static BYTES: AtomicUsize = AtomicUsize::new(0);
static WIRE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PICTURE_PARTS: std::sync::Mutex<libbreenix::evidence_reference::Cache> =
    std::sync::Mutex::new(libbreenix::evidence_reference::Cache::new());
static UPLOADS: AtomicUsize = AtomicUsize::new(0);
pub(super) fn totals() -> (usize, usize) { (BYTES.load(Relaxed), UPLOADS.load(Relaxed)) }

use libbreenix::{crypto::sha256::sha256,dns,io::{self,PollFd,poll_events},socket,time};
pub fn upload()->Result<(),String> { upload_slot(None, false) }
pub(super) fn upload_slot(resources: Option<&libbreenix::gpu::abi::Caps>, final_slot: bool)->Result<(),String> {
    upload_slot_with(resources, final_slot, &[])
}
pub(super) fn upload_slot_with(resources: Option<&libbreenix::gpu::abi::Caps>, final_slot: bool, extra: &[u8])->Result<(),String> {
    upload_slot_records(resources, final_slot, extra, &[], &[])
}
pub(super) fn upload_slot_records(resources: Option<&libbreenix::gpu::abi::Caps>, final_slot: bool, extra: &[u8], pictures: &[u8], allocations: &[u8])->Result<(),String> {
    let resource_row = resources.map(|caps| {
        // Caps implements Plain: repr(C), integer fields only, no padding.
        // Retain the exact syscall bytes; do not reconstruct admission here.
        let bytes = unsafe { core::slice::from_raw_parts(
            (caps as *const libbreenix::gpu::abi::Caps).cast::<u8>(),
            core::mem::size_of_val(caps)) };
        let hex: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
        let frames = std::fs::read_to_string("/proc/breenix/frames").unwrap_or_default();
        let frame_census_hex: String = frames.as_bytes().iter().map(|v| format!("{v:02x}")).collect();
        let check = std::fs::read_to_string("/proc/console/check").unwrap_or_default();
        let console_check_hex: String = check.as_bytes().iter().map(|v| format!("{v:02x}")).collect();
        format!("GT710_RESOURCES {{\"schema\":1,\"type\":\"caps\",\"bytes\":512,\"hex\":\"{hex}\",\"frame_census_hex\":\"{frame_census_hex}\",\"console_check_hex\":\"{console_check_hex}\"}}\n")
    });
    let mut data=std::fs::read("/proc/gt710-pixels").unwrap_or_default();
    if let Some(row) = &resource_row { data.extend_from_slice(row.as_bytes()); }
    if !extra.is_empty() {
        if !final_slot { return Err("case heap evidence requires the final upload slot".into()); }
        data.try_reserve_exact(extra.len()).map_err(|_| "native final artifact allocation")?;
        data.extend_from_slice(extra);
    }
    if !pictures.is_empty() {
        if !final_slot { return Err("picture manifest requires the final upload slot".into()); }
        data.try_reserve_exact(pictures.len()).map_err(|_| "picture manifest allocation")?;
        data.extend_from_slice(pictures);
    }
    if !allocations.is_empty() {
        if !final_slot { return Err("allocation census requires the final slot".into()); }
        data.try_reserve_exact(allocations.len()).map_err(|_| "allocation census artifact allocation")?;
        data.extend_from_slice(allocations);
    }
    if data.is_empty() {return Ok(());}
    if data.len()>MAX_UPLOAD {return Err("native display artifact exceeds bound".into());}
    let suffix = if final_slot { "/final" } else { "" };
    transfer(&data, suffix)?;
    let digest:String=sha256(&data).iter().map(|b|format!("{b:02x}")).collect();
    if let Some(row) = resource_row {
        let row_digest: String = sha256(row.as_bytes()).iter().map(|b|format!("{b:02x}")).collect();
        println!("GT710_RESOURCES_BULK {} {}", row.len(), row_digest);
    }
    if !extra.is_empty() {
        let extra_digest: String = sha256(extra).iter().map(|b|format!("{b:02x}")).collect();
        println!("GT710_CASE_HEAP_BULK {} {}", extra.len(), extra_digest);
    }
    if !pictures.is_empty() {
        let digest: String = sha256(pictures).iter().map(|b| format!("{b:02x}")).collect();
        println!("GT710_BREENGL_MANIFEST_BULK {} {digest}", pictures.len());
    }
    if !allocations.is_empty() {
        let digest: String = sha256(allocations).iter().map(|b| format!("{b:02x}")).collect();
        println!("GT710_ALLOCATIONS_BULK {} {digest}", allocations.len());
    }
    let anchor=if final_slot {"GT710_FINAL_BULK"} else {"GT710_BULK"};
    println!("{} {} {}",anchor,data.len(),digest);
    Ok(())
}

/// Send a bounded slot body. BreenGL supplies /pictures/N; startup/final retain G12.
pub(super) fn transfer(data: &[u8], suffix: &str) -> Result<(), String> {
    let (total, _) = totals();
    if data.is_empty() || data.len() > MAX_UPLOAD || total.checked_add(data.len()).is_none_or(|n| n > MAX_TOTAL) {
        return Err(format!("native upload refused: slot bytes={} total={} bound={MAX_TOTAL}", data.len(), total));
    }
    let data_digest=sha256(data);
    let suite=std::env::args().next().and_then(|name|name.rsplit('/').next()?.strip_prefix("suite-").map(str::to_owned));
    let part=suffix.strip_prefix("/pictures/").and_then(|n|n.parse::<usize>().ok());
    let proof=suite.as_deref().is_some_and(breenix_suite::suite::screen_proof_enabled);
    let mut parts=PICTURE_PARTS.lock().map_err(|_|"picture reference cache poisoned")?;
    let reference=if proof { part.and_then(|index|parts.reference(index,data.len(),data_digest)) } else { None };
    let body=reference.as_deref().map_or(data,str::as_bytes);
    if WIRE_BYTES.load(Relaxed).checked_add(body.len()).is_none_or(|n|n>MAX_TOTAL) {
        return Err("native wire evidence exceeds 64 MiB".into());
    }
    let mut target=std::fs::read_to_string("/etc/breenix/gt710-upload").map_err(|_|"native upload route absent")?.trim().to_string();
    if std::fs::read_to_string("/etc/breenix/suite-sequence").is_ok_and(|s| s.trim().contains(',')) {
        let executable = std::env::args().next().ok_or("native suite executable absent")?;
        let name = executable.rsplit('/').next().unwrap_or("");
        let suite = name.strip_prefix("suite-").ok_or("native suite name absent")?;
        if !matches!(suite, "console" | "gpu" | "breengel") { return Err("native suite name refused".into()); }
        target.push_str("/suites/");
        target.push_str(suite);
    }
    target.push_str(suffix);
    let rest=target.trim().strip_prefix("http://").ok_or("native upload needs HTTP")?;
    let (authority,path)=rest.split_once('/').ok_or("native upload path absent")?;
    let (host,port)=authority.rsplit_once(':').map_or((authority,80),|(h,p)|(h,p.parse().unwrap_or(0)));
    if port==0 || authority.contains('@') || authority.contains(['\r','\n']) || !path.starts_with("gt710-upload/") {
        return Err("invalid native upload authority".into());
    }
    let start=time::now_monotonic().map_err(|_|"native upload clock")?;
    for _ in 0..200 {
        if std::fs::read_to_string("/proc/net/pnp").is_ok_and(|s|s.lines().any(|l|l.starts_with("bootserver "))) {break;}
        time::sleep_ms(100).map_err(|_|"native lease wait")?;
    }
    let ip=match host.parse::<std::net::Ipv4Addr>() {
        Ok(address)=>address.octets(),
        Err(_)=>dns::resolve_auto(host).map_err(|_|"native upload resolution")?.addr,
    };
    let fd=socket::socket(socket::AF_INET,socket::SOCK_STREAM,0).map_err(|_|"native upload socket")?;
    let wire_digest=if reference.is_some() { sha256(body) } else { data_digest };
    let digest:String=wire_digest.iter().map(|b|format!("{b:02x}")).collect();
    WIRE_BYTES.fetch_add(body.len(), Relaxed);
    let result=(|| {
        socket::connect_inet(fd,&socket::SockAddrIn::new(ip,port)).map_err(|_|"native upload connect")?;
        let reference_header=if reference.is_some() { "X-Breenix-Picture-Reference: 1\r\n" } else { "" };
        let header=format!("PUT /{} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nX-Content-SHA256: {}\r\n{}Connection: close\r\n\r\n",path,authority,body.len(),digest,reference_header);
        for bytes in [header.as_bytes(),body] {
            let mut offset=0;
            while offset<bytes.len() {
                let n=socket::send(fd,&bytes[offset..(offset+65536).min(bytes.len())]).map_err(|_|"native artifact send")?;
                if n==0 {return Err("native artifact send stopped");}
                offset+=n;
            }
        }
        let mut response=Vec::new();
        while response.len()<4096 && !response.windows(4).any(|w|w==b"\r\n\r\n") {
            let mut poll=[PollFd::new(fd,poll_events::POLLIN)];
            if io::poll(&mut poll,10000).map_err(|_|"native upload response poll")?==0 {return Err("native upload response timeout");}
            let mut bytes=[0u8;512];let n=socket::recv(fd,&mut bytes).map_err(|_|"native upload response")?;
            if n==0 {break;}response.extend_from_slice(&bytes[..n]);
        }
        if !response.starts_with(b"HTTP/1.1 200 ") || !response.windows(4).any(|w|w==b"\r\n\r\n") {
            return Err("native artifact not accepted");
        }
        Ok(())
    })();
    let _=io::close(fd);
    result.map_err(str::to_string)?;
    if proof { if let Some(index)=part { parts.remember(index,data.len(),data_digest).map_err(str::to_string)?; } }
    BYTES.fetch_add(data.len(), Relaxed);
    UPLOADS.fetch_add(1, Relaxed);
    let end=time::now_monotonic().map_err(|_|"native upload clock")?;
    let micros=|t:libbreenix::types::Timespec|t.tv_sec as i128*1_000_000+t.tv_nsec as i128/1000;
    println!("GT710_PROBE_TIME network-upload us={}",micros(end)-micros(start));
    Ok(())
}
