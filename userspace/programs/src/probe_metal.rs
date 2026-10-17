//! The site config names a known boot-server file and its expected SHA-256.
use libbreenix::{crypto::sha256::sha256, http, time};

pub fn fetch() -> Result<(), &'static str> {
    let site =
        std::fs::read_to_string("/etc/breenix/site.env").map_err(|_| "site config unreadable")?;
    let value = |key: &str| site.lines().find_map(|line| line.strip_prefix(key));
    let url = value("METAL_FETCH_URL=").ok_or("missing fetch URL")?;
    let expected = value("METAL_FETCH_SHA256=").ok_or("missing fetch checksum")?;
    if !url.starts_with("http://") || expected.len() != 64 {
        return Err("invalid fetch configuration");
    }
    // Link negotiation, DHCP retransmission and address conflict detection
    // finish independently of userspace startup; wait for the live lease.
    for _ in 0..200 {
        if libbreenix::dns::dhcp_nameserver().is_some() {
            break;
        }
        time::sleep_ms(100).map_err(|_| "lease wait failed")?;
    }
    if libbreenix::dns::dhcp_nameserver().is_none() {
        return Err("no DHCP lease");
    }
    let mut bytes = vec![0; 132 * 1024];
    let (response, received) =
        http::http_get_with_buf(url, &mut bytes).map_err(|_| "TCP fetch failed")?;
    if response.status_code != 200 || response.body_len != 128 * 1024 {
        return Err("unexpected HTTP status or file length");
    }
    let end = response
        .body_offset
        .checked_add(response.body_len)
        .ok_or("body length overflow")?;
    let body = bytes
        .get(response.body_offset..end)
        .filter(|_| end <= received)
        .ok_or("truncated HTTP body")?;
    let actual: String = sha256(body)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err("file SHA-256 mismatch");
    }
    Ok(())
}
