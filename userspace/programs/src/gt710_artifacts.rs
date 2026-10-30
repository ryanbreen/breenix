//! Send actual compressed readback rows to the boot service; serial keeps their digest.
use libbreenix::{crypto::sha256::sha256,dns,io::{self,PollFd,poll_events},socket,time};
pub fn upload()->Result<(),String> { upload_slot(None, false) }
pub(super) fn upload_slot(resources: Option<&libbreenix::gpu::abi::Caps>, final_slot: bool)->Result<(),String> {
    upload_slot_with(resources, final_slot, &[])
}
pub(super) fn upload_slot_with(resources: Option<&libbreenix::gpu::abi::Caps>, final_slot: bool, extra: &[u8])->Result<(),String> {
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
    if data.is_empty() {return Ok(());}
    if data.len()>2<<20 {return Err("native display artifact exceeds bound".into());}
    let mut target=std::fs::read_to_string("/etc/breenix/gt710-upload").map_err(|_|"native upload route absent")?.trim().to_string();
    if final_slot { target.push_str("/final"); }
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
    let digest:String=sha256(&data).iter().map(|b|format!("{b:02x}")).collect();
    let result=(|| {
        socket::connect_inet(fd,&socket::SockAddrIn::new(ip,port)).map_err(|_|"native upload connect")?;
        let header=format!("PUT /{} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nX-Content-SHA256: {}\r\nConnection: close\r\n\r\n",path,authority,data.len(),digest);
        for bytes in [header.as_bytes(),data.as_slice()] {
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
    let end=time::now_monotonic().map_err(|_|"native upload clock")?;
    let micros=|t:libbreenix::types::Timespec|t.tv_sec as i128*1_000_000+t.tv_nsec as i128/1000;
    if let Some(row) = resource_row {
        let row_digest: String = sha256(row.as_bytes()).iter().map(|b|format!("{b:02x}")).collect();
        println!("GT710_RESOURCES_BULK {} {}", row.len(), row_digest);
    }
    let anchor=if final_slot {"GT710_FINAL_BULK"} else {"GT710_BULK"};
    println!("{} {} {}",anchor,data.len(),digest);
    println!("GT710_PROBE_TIME network-upload us={}",micros(end)-micros(start));
    Ok(())
}
