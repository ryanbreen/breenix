//! DNS resolution test for bcheck.
//!
//! Resolves "example.com" using auto-detected DNS server.
//! Exits 0 on success, 1 on failure.

use libbreenix::dns;
use std::process;

fn main() {
    println!("[net_test] Resolving example.com...");

    match dns::resolve_auto("example.com") {
        Ok(result) => {
            let ip = result.addr;
            println!(
                "[net_test] PASS -> {}.{}.{}.{}",
                ip[0], ip[1], ip[2], ip[3]
            );
            process::exit(0);
        }
        Err(e) => {
            println!("[net_test] FAIL ({:?})", e);
            process::exit(1);
        }
    }
}
