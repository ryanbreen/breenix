//! Supervised SNTP client. Sources are site configuration or DHCP option 42.
//! RFC 4330/5905 wire behavior; first synchronization steps, later ones slew.
mod sntp;

use libbreenix::socket::{
    bind_inet, recvfrom, sendto, socket, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK,
};
use libbreenix::syscall::{nr, raw};
use libbreenix::types::Timespec;
use libbreenix::{dns, time};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

const STATE: &str = "/run/breenix/ntp-state";
const STEPPED: &str = "/run/breenix/ntp-stepped";
const POLL: u64 = 3600;

fn mono() -> u64 {
    let ts = time::now_monotonic().expect("monotonic clock");
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
fn real() -> i128 {
    let ts = time::now_realtime().expect("realtime clock");
    i128::from(ts.tv_sec) * sntp::NS + i128::from(ts.tv_nsec)
}
fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
fn syscall(number: u64, a: u64, b: u64, c: u64) -> Result<(), String> {
    let result = unsafe { raw::syscall3(number, a, b, c) } as i64;
    if result < 0 {
        Err(format!("syscall {number}: {}", -result))
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct State {
    syncs: u64,
    first: u64,
    last: u64,
    source: String,
}
impl State {
    fn load() -> Self {
        let text = fs::read_to_string(STATE).unwrap_or_default();
        let fields: Vec<_> = text.split_whitespace().collect();
        if fields.len() != 4 {
            return Self::default();
        }
        Self {
            syncs: fields[0].parse().unwrap_or(0),
            first: fields[1].parse().unwrap_or(0),
            last: fields[2].parse().unwrap_or(0),
            source: fields[3].to_owned(),
        }
    }
    fn save(&self) -> Result<(), String> {
        fs::write(
            format!("{STATE}.new"),
            format!(
                "{} {} {} {}\n",
                self.syncs, self.first, self.last, self.source
            ),
        )
        .map_err(|e| e.to_string())?;
        fs::rename(format!("{STATE}.new"), STATE).map_err(|e| e.to_string())
    }
}

fn rtc_read() -> Result<[i32; 9], String> {
    let fd = libbreenix::fs::open("/dev/rtc0", 0).map_err(|e| format!("RTC open: {e:?}"))?;
    let mut tm = [0i32; 9];
    let result = syscall(nr::IOCTL, fd.raw(), 0x8024_7009, tm.as_mut_ptr() as u64);
    let _ = syscall(nr::CLOSE, fd.raw(), 0, 0);
    result.map(|()| tm)
}
fn rtc_write() -> Result<(), String> {
    // Align the integer calendar to a realtime seconds edge rather than
    // discarding up to one second when loading a fractional wall clock.
    loop {
        let now = time::now_realtime().map_err(|e| format!("realtime: {e:?}"))?;
        if now.tv_nsec >= 990_000_000 {
            while real() / sntp::NS == i128::from(now.tv_sec) {
                std::hint::spin_loop();
            }
            break;
        }
        sleep(10);
    }
    let secs = (real() / sntp::NS) as u64;
    let days = secs / 86400;
    let mut year = 1970;
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mut day = days;
    while day >= if leap(year) { 366 } else { 365 } {
        day -= if leap(year) { 366 } else { 365 };
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while day >= months[month] {
        day -= months[month];
        month += 1;
    }
    let tm = [
        (secs % 60) as i32,
        (secs / 60 % 60) as i32,
        (secs / 3600 % 24) as i32,
        day as i32 + 1,
        month as i32,
        year - 1900,
        0,
        0,
        0,
    ];
    let fd = libbreenix::fs::open("/dev/rtc0", 0).map_err(|e| format!("RTC open: {e:?}"))?;
    let result = syscall(nr::IOCTL, fd.raw(), 0x4024_700a, tm.as_ptr() as u64);
    let _ = syscall(nr::CLOSE, fd.raw(), 0, 0);
    result
}
fn status() -> Result<(), String> {
    // Bracket realtime with monotonic reads to limit measurement skew.
    let a = mono();
    let wall = real();
    let b = mono();
    let state = State::load();
    let rtc = rtc_read()?;
    println!("{{\"realtime_ns\":{wall},\"monotonic_ns\":{},\"syncs\":{},\"first_ms\":{},\"last_ms\":{},\"source\":\"{}\",\"steps\":{},\"poll_seconds\":3600,\"rtc_utc\":{},\"rtc\":{:?}}}",
        a + (b-a)/2, state.syncs, state.first, state.last, state.source,
        u8::from(Path::new(STEPPED).exists()), state.syncs > 0, rtc);
    Ok(())
}
fn sources() -> Result<(Vec<String>, &'static str), String> {
    let site = fs::read_to_string("/etc/breenix/sntp-servers").unwrap_or_default();
    let names: Vec<_> = site
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect();
    if !names.is_empty() {
        return Ok((names, "site"));
    }
    let pnp = fs::read_to_string("/proc/net/pnp").map_err(|e| e.to_string())?;
    let names: Vec<_> = pnp
        .lines()
        .filter_map(|l| l.strip_prefix("ntpserver "))
        .map(str::to_owned)
        .collect();
    if names.is_empty() {
        Err("no site or DHCP SNTP servers".into())
    } else {
        Ok((names, "dhcp"))
    }
}
fn resolve(name: &str) -> Result<[u8; 4], String> {
    let parts: Vec<_> = name.split('.').map(str::parse::<u8>).collect();
    if parts.len() == 4 && parts.iter().all(Result::is_ok) {
        return Ok([
            *parts[0].as_ref().unwrap(),
            *parts[1].as_ref().unwrap(),
            *parts[2].as_ref().unwrap(),
            *parts[3].as_ref().unwrap(),
        ]);
    }
    dns::resolve_auto(name)
        .map(|r| r.addr)
        .map_err(|e| format!("DNS: {e:?}"))
}

fn query(name: &str, port: u16) -> Result<i64, String> {
    let ip = resolve(name)?;
    // Persist per-endpoint minimum intervals across daemon restarts and --once.
    // A KoD RATE extends it, DENY/RSTR disables this endpoint for this boot.
    let peer = format!(
        "/run/breenix/ntp-peer-{}-{}-{}-{}-{port}",
        ip[0], ip[1], ip[2], ip[3]
    );
    let now = mono();
    let due = fs::read_to_string(&peer)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if now < due {
        return Err("server polling backoff".into());
    }
    fs::write(&peer, (now.saturating_add(16_000_000_000)).to_string())
        .map_err(|e| e.to_string())?;
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0).map_err(|e| format!("UDP: {e:?}"))?;
    let result = (|| {
        bind_inet(fd, &SockAddrIn::new([0; 4], 0)).map_err(|e| format!("bind: {e:?}"))?;
        let sent = real();
        let start = mono();
        let mut origin = sntp::timestamp(sent);
        // Low fractional bits carry a fresh random nonce, with <16 us error.
        let mut entropy = [0u8; 2];
        let filled = libbreenix::crypto::rand::getrandom_bytes(&mut entropy)
            .map_err(|errno| format!("getrandom: {}", -errno))?;
        if filled != entropy.len() {
            return Err("short getrandom result".into());
        }
        origin[6..].copy_from_slice(&entropy);
        let mut request = [0u8; 48];
        request[0] = 0x23;
        request[2] = 12;
        request[40..48].copy_from_slice(&origin);
        let destination = SockAddrIn::new(ip, port);
        sendto(fd, &request, &destination).map_err(|e| format!("send: {e:?}"))?;
        let mut packet = [0u8; 512];
        while mono().saturating_sub(start) < 5_000_000_000 {
            let mut from = SockAddrIn::default();
            if let Ok(len) = recvfrom(fd, &mut packet, Some(&mut from)) {
                let elapsed = mono().saturating_sub(start);
                if from.addr != ip || from.port != destination.port {
                    continue;
                }
                return match sntp::response(
                    &packet[..len],
                    origin,
                    sent,
                    elapsed,
                    Path::new(STEPPED).exists(),
                )
                .map_err(str::to_owned)?
                {
                    sntp::Reply::Offset(offset) => Ok(offset),
                    sntp::Reply::Rate(seconds) => {
                        fs::write(
                            &peer,
                            now.saturating_add(seconds.saturating_mul(1_000_000_000))
                                .to_string(),
                        )
                        .map_err(|e| e.to_string())?;
                        Err("KoD RATE".into())
                    }
                    sntp::Reply::Denied => {
                        fs::write(&peer, u64::MAX.to_string()).map_err(|e| e.to_string())?;
                        Err("KoD DENY/RSTR".into())
                    }
                };
            }
            sleep(10);
        }
        Err("SNTP timeout".into())
    })();
    let _ = syscall(nr::CLOSE, fd.raw(), 0, 0);
    result
}

fn synchronize(server: &str, port: u16, source: &str) -> Result<(), String> {
    let offset = query(server, port)?;
    // A marker published before the first step prevents a restarted client
    // from stepping a second time, including after a partial synchronization.
    if !Path::new(STEPPED).exists() {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(STEPPED)
            .map_err(|e| e.to_string())?;
        let now = real() + i128::from(offset);
        let ts = Timespec {
            tv_sec: (now / sntp::NS) as i64,
            tv_nsec: (now % sntp::NS) as i64,
        };
        time::clock_settime(time::CLOCK_REALTIME, &ts).map_err(|e| format!("clock step: {e:?}"))?;
    } else {
        let mut tx = [0i64; 26];
        tx[0] = 0x8001;
        tx[1] = offset / 1000;
        #[cfg(target_arch = "x86_64")]
        const ADJTIMEX: u64 = 159;
        #[cfg(target_arch = "aarch64")]
        const ADJTIMEX: u64 = 171;
        syscall(ADJTIMEX, tx.as_mut_ptr() as u64, 0, 0)?;
    }
    rtc_write()?;
    let mut state = State::load();
    let now = mono() / 1_000_000;
    if state.syncs == 0 {
        state.first = now;
    }
    state.syncs += 1;
    state.last = now;
    state.source = source.into();
    state.save()?;
    println!(
        "[ntp] synchronized count={} monotonic_ms={} offset_ns={} method={} rtc=UTC poll=3600",
        state.syncs,
        now,
        offset,
        if state.syncs == 1 { "step" } else { "slew" }
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--status"] {
        return status();
    }
    if args.first().map(String::as_str) == Some("--watch") && args.len() == 2 {
        let seconds: u64 = args[1].parse().map_err(|_| "watch seconds")?;
        if !(1..=60).contains(&seconds) {
            return Err("watch 1..60 seconds".into());
        }
        let start = mono();
        while mono() - start < seconds * 1_000_000_000 {
            status()?;
            sleep(10);
        }
        return Ok(());
    }
    let once = args.first().map(String::as_str) == Some("--once");
    let mut server = None;
    let mut port = 123;
    let mut at = usize::from(once);
    while at < args.len() {
        match args[at].as_str() {
            "--server" if at+1 < args.len() => server = Some(args[at+1].clone()),
            "--port" if at+1 < args.len() => port = args[at+1].parse().map_err(|_| "port")?,
            _ => return Err("usage: ntp_client [--once] [--server NAME] [--port PORT] | --status | --watch SECONDS".into()),
        }
        at += 2;
    }
    #[cfg(target_arch = "x86_64")]
    const GETEUID: u64 = 107;
    #[cfg(target_arch = "aarch64")]
    const GETEUID: u64 = 175;
    let uid = unsafe { raw::syscall0(GETEUID) };
    if uid != 0 {
        return Err("root required to synchronize time".into());
    }
    fs::create_dir_all("/run/breenix").map_err(|e| e.to_string())?;
    // Advisory record lock releases automatically on process death.
    use std::os::fd::AsRawFd;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open("/run/breenix/ntp-lock")
        .map_err(|e| e.to_string())?;
    #[repr(C)]
    struct Lock {
        kind: i16,
        whence: i16,
        start: i64,
        len: i64,
        pid: i32,
    }
    let fl = Lock {
        kind: 1,
        whence: 0,
        start: 0,
        len: 0,
        pid: 0,
    };
    syscall(
        nr::FCNTL,
        lock.as_raw_fd() as u64,
        6,
        &fl as *const Lock as u64,
    )?;
    let mut retry = 16;
    let state = State::load();
    let mut due = if once || state.syncs == 0 {
        0
    } else {
        state
            .last
            .saturating_mul(1_000_000)
            .saturating_add(POLL * 1_000_000_000)
    };
    loop {
        while mono() < due {
            sleep(1000.min(due.saturating_sub(mono()) / 1_000_000 + 1));
        }
        let (names, source) = match &server {
            Some(name) => (vec![name.clone()], "manual"),
            None => sources()?,
        };
        let attempted = mono();
        let mut success = false;
        for name in names.iter().take(8) {
            match synchronize(name, port, source) {
                Ok(()) => {
                    success = true;
                    break;
                }
                Err(error) => eprintln!("[ntp] query refused: {error}"),
            }
        }
        if once {
            return if success {
                Ok(())
            } else {
                Err("no acceptable SNTP reply".into())
            };
        }
        let interval = if success {
            retry = 16;
            POLL
        } else {
            let seconds = retry;
            retry = (retry * 2).min(POLL);
            seconds
        };
        due = (if success {
            State::load().last.saturating_mul(1_000_000)
        } else {
            attempted
        })
        .saturating_add(interval * 1_000_000_000);
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("[ntp] {error}");
        std::process::exit(1);
    }
}
