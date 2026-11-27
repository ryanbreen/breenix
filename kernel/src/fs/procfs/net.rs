//! Bounded, fallible network configuration snapshots.
use crate::fs::attrs::InodeAttrs;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

pub(super) const RECEIVE_COUNTERS_PATH: &str = "/proc/net/rx_counters";

pub fn receive_counters_attrs(path: &str) -> Option<InodeAttrs> {
    const WHEEL_GID: u32 = 10;
    (path == RECEIVE_COUNTERS_PATH).then_some(InodeAttrs::new(0o100440, 0, WHEEL_GID))
}

pub(super) fn list_entries() -> Result<Vec<String>, i32> {
    let mut entries = Vec::new();
    entries.try_reserve_exact(3).map_err(|_| -12)?;
    for listed in ["pnp", "tcp", "rx_counters"] {
        let mut name = String::new();
        name.try_reserve_exact(listed.len()).map_err(|_| -12)?;
        name.push_str(listed);
        entries.push(name);
    }
    Ok(entries)
}

pub(super) fn generate_pnp() -> Result<String, i32> {
    let mut text = String::new();
    // Three resolver addresses, three time sources and one server fit in 256 bytes.
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

pub(super) fn generate_receive_counters() -> Result<String, i32> {
    let mut text = String::new();
    // Ten rows at most, each with one decimal u64.
    text.try_reserve_exact(512).map_err(|_| -12)?;
    append_receive_counters(&mut text);
    Ok(text)
}

pub(super) fn append_receive_counters(output: &mut String) {
    let stats = crate::net::tcp::receive_counters();
    let _ = writeln!(output, "tcp_rx_checksum_rejected: {}", stats.checksum_rejected);
    let _ = writeln!(output, "tcp_rx_outside_window: {}", stats.outside_window);
    let _ = writeln!(output, "tcp_rx_gap_segments: {}", stats.gap_segments);
    let _ = writeln!(output, "tcp_rx_ack_output_failed: {}", stats.ack_output_failed);

    #[cfg(target_arch = "x86_64")]
    if let Some(stats) = crate::drivers::bcm5720::receive_counters() {
        let _ = writeln!(output, "bcm5720_msi: {}", stats.msi);
        let _ = writeln!(output, "bcm5720_rx_no_bd: {}", stats.no_rx_bd);
        let _ = writeln!(output, "bcm5720_rx_discards: {}", stats.in_discards);
        let _ = writeln!(output, "bcm5720_rx_errors: {}", stats.in_errors);
        let _ = writeln!(output, "bcm5720_rx_bad_completions: {}", stats.bad_completions);
        let _ = writeln!(output, "bcm5720_tx_ring_full: {}", stats.tx_ring_full);
    }
}
