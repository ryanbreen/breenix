//! Exercise the real DNS client against a local UDP resolver fixture.
//! Loopback works with every NIC profile, including a disconnected VMware NIC.
//! A missing response, wrong answer, wrong RCODE or repeated transaction ID fails.
use libbreenix::dns::{self, DnsError};
use libbreenix::process::{fork, waitpid, wexitstatus, wifexited, yield_now, ForkResult};
use libbreenix::signal::{kill, SIGKILL};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK};
use std::time::{Duration, Instant};

const LOCAL: [u8; 4] = [127, 0, 0, 1];
const ANSWER: [u8; 4] = [192, 0, 2, 42];

struct Resolver(i32);
impl Drop for Resolver {
    fn drop(&mut self) {
        if self.0 != 0 {
            let _ = kill(self.0, SIGKILL);
            let mut status = 0;
            let _ = waitpid(self.0, &mut status, 0);
        }
    }
}

fn serve(fd: libbreenix::types::Fd) -> Result<(), &'static str> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut previous_id = None;
    for index in 0..5 {
        let mut query = [0u8; dns::DNS_BUF_SIZE];
        let mut source = SockAddrIn::default();
        let len = loop {
            match socket::recvfrom(fd, &mut query, Some(&mut source)) {
                Ok(len) => break len,
                Err(_) if Instant::now() < deadline => { let _ = yield_now(); }
                Err(_) => return Err("resolver receive deadline"),
            }
        };
        let id = u16::from_be_bytes([query[0], query[1]]);
        let hostname = match index {
            0 => "www.breenix.test",
            2 => "missing.breenix.test",
            _ => "example.breenix.test",
        };
        let mut expected = [0u8; dns::DNS_BUF_SIZE];
        let expected_len = dns::encode_query(hostname, id, &mut expected);
        if len != expected_len || query[..len] != expected[..len] {
            return Err("resolver received an incorrect question");
        }
        if index == 4 && previous_id == Some(id) {
            return Err("transaction ID did not change");
        }
        previous_id = Some(id);
        query[2] = 0x81;
        query[3] = if index == 2 { 0x83 } else { 0x80 };
        query[7] = if index == 2 { 0 } else { 1 };
        let mut response_len = len;
        if index != 2 {
            // Compressed name, A/IN, TTL 60, four octets of RDATA.
            let answer = [0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4,
                ANSWER[0], ANSWER[1], ANSWER[2], ANSWER[3]];
            query[len..len + answer.len()].copy_from_slice(&answer);
            response_len += answer.len();
        }
        if socket::sendto(fd, &query[..response_len], &source).ok() != Some(response_len) {
            return Err("resolver send failed");
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let fd = socket::socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0)
        .map_err(|e| format!("resolver socket: {e:?}"))?;
    socket::bind_inet(fd, &SockAddrIn::new(LOCAL, dns::DNS_PORT))
        .map_err(|e| format!("resolver bind: {e:?}"))?;
    let mut resolver = match fork().map_err(|e| format!("resolver fork: {e:?}"))? {
        ForkResult::Child => {
            let result = serve(fd);
            let _ = libbreenix::io::close(fd);
            if let Err(message) = result { eprintln!("DNS_TEST: fixture FAILED {message}"); }
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
        ForkResult::Parent(pid) => Resolver(pid.raw() as i32),
    };
    let _ = libbreenix::io::close(fd);
    for (name, hostname) in [("local_resolve", "www.breenix.test"),
        ("example_resolve", "example.breenix.test")] {
        let result = dns::resolve(hostname, LOCAL).map_err(|e| format!("{name}: {e:?}"))?;
        if result.addr != ANSWER || result.ttl != 60 { return Err(format!("{name}: wrong answer")); }
        println!("DNS_TEST: {name} OK");
    }
    if !matches!(dns::resolve("missing.breenix.test", LOCAL), Err(DnsError::ServerError(3))) {
        return Err("NXDOMAIN did not return RCODE 3".into());
    }
    println!("DNS_TEST: nxdomain OK");
    if !matches!(dns::resolve("", LOCAL), Err(DnsError::InvalidHostname)) {
        return Err("empty hostname accepted".into());
    }
    println!("DNS_TEST: empty_hostname OK");
    if !matches!(dns::resolve(&"a".repeat(256), LOCAL), Err(DnsError::HostnameTooLong)) {
        return Err("long hostname accepted".into());
    }
    println!("DNS_TEST: long_hostname OK");
    for _ in 0..2 {
        let answer = dns::resolve("example.breenix.test", LOCAL)
            .map_err(|e| format!("transaction query: {e:?}"))?;
        if answer.addr != ANSWER { return Err("transaction query returned wrong answer".into()); }
    }
    let mut status = 0;
    let reaped = waitpid(resolver.0, &mut status, 0).map_err(|e| format!("resolver wait: {e:?}"))?;
    if reaped.raw() as i32 != resolver.0 || !wifexited(status) || wexitstatus(status) != 0 {
        return Err("resolver failed its question/transaction assertions".into());
    }
    resolver.0 = 0;
    println!("DNS_TEST: txid_varies OK");
    Ok(())
}

fn main() {
    println!("DNS Test: Starting local resolver");
    match run() {
        Ok(()) => println!("DNS Test: All tests passed!"),
        Err(message) => { eprintln!("DNS_TEST: FAILED {message}"); std::process::exit(1); }
    }
}
