//! Send actual compressed readback rows to the boot service; serial keeps their digest.
use libbreenix::{crypto::sha256::sha256,dns,io::{self,PollFd,poll_events},socket,time};
pub fn upload()->Result<(),String> {
    let data=std::fs::read("/proc/gt710-pixels").unwrap_or_default();
    if data.is_empty() {return Ok(());}
    if data.len()>2<<20 {return Err("native display artifact exceeds bound".into());}
    let target=std::fs::read_to_string("/etc/breenix/gt710-upload").map_err(|_|"native upload route absent")?;
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
    let ip=dns::resolve_auto(host).map_err(|_|"native upload resolution")?.addr;
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
    println!("GT710_BULK {} {}",data.len(),digest);
    println!("GT710_PROBE_TIME network-upload us={}",micros(end)-micros(start));
    Ok(())
}
