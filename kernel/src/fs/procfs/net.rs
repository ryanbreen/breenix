//! Bounded, fallible network configuration snapshots.
use alloc::string::String;
use alloc::vec::Vec;

pub(super) fn list_entries() -> Result<Vec<String>, i32> {
    let mut entries = Vec::new();
    entries.try_reserve_exact(2).map_err(|_| -12)?;
    for listed in ["pnp", "tcp"] {
        let mut name = String::new();
        name.try_reserve_exact(listed.len()).map_err(|_| -12)?;
        name.push_str(listed);
        entries.push(name);
    }
    Ok(entries)
}

pub(super) fn generate_pnp() -> Result<String, i32> {
    use core::fmt::Write;
    let mut text = String::new();
    // Three resolver addresses and one server need at most 128 bytes.
    text.try_reserve_exact(256).map_err(|_| -12)?;
    match crate::net::dhcp::lease() {
        Some(lease) => {
            text.push_str("#PROTO: DHCP\n");
            for [a, b, c, d] in lease.nameservers.into_iter().flatten() {
                let _ = writeln!(text, "nameserver {}.{}.{}.{}", a, b, c, d);
            }
            for [a, b, c, d] in lease.ntp_servers.into_iter().flatten() {
                let _ = writeln!(text, "ntpserver {}.{}.{}.{}", a, b, c, d);
            }
            let [a, b, c, d] = lease.server;
            let _ = writeln!(text, "bootserver {}.{}.{}.{}", a, b, c, d);
        }
        None if crate::net::dhcp::configuration_required() => text.push_str("#PROTO: DHCP\n"),
        None => text.push_str("#MANUAL\n"),
    }
    Ok(text)
}
