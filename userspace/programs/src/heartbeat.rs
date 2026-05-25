//! Minimal non-GPU userspace heartbeat for scheduler liveness diagnosis.

use libbreenix::fs::{self, O_RDONLY};
use libbreenix::io;
use libbreenix::process::gettid;
use libbreenix::time::{now_monotonic, sleep_ms};

const COUNTER_PATH: &str = "/proc/trace/counters";
const NET_RX_COUNTER_PREFIXES: &[&str] = &[
    "NET_RX_MSI_TOTAL:",
    "NET_RX_RING_DRAIN_TOTAL:",
    "NET_RX_FRAME_TOTAL:",
    "NET_RX_ARP_TOTAL:",
    "NET_RX_ETHERTYPE_OTHER_TOTAL:",
    "NET_PCI_IRQ_RAISED_NETRX:",
];

fn monotonic_ms() -> u64 {
    now_monotonic()
        .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
        .unwrap_or(0)
}

fn dump_net_rx_counters(sample: u32) {
    let fd = match fs::open(COUNTER_PATH, O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => {
            print!("[net-rx-counters] sample={} open failed: {}\n", sample, e);
            return;
        }
    };

    let mut buf = [0u8; 4096];
    let n = match io::read(fd, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            let _ = io::close(fd);
            print!("[net-rx-counters] sample={} read failed: {}\n", sample, e);
            return;
        }
    };
    let _ = io::close(fd);

    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    print!("[net-rx-counters] sample={} begin\n", sample);
    for line in text.lines() {
        if NET_RX_COUNTER_PREFIXES
            .iter()
            .any(|prefix| line.starts_with(prefix))
        {
            print!("[net-rx-counters] sample={} {}\n", sample, line);
        }
    }
    print!("[net-rx-counters] sample={} end\n", sample);
}

fn main() {
    let tid = gettid().map(|tid| tid.raw()).unwrap_or(0);
    let mut next_dump_ms = 20_000;
    let mut sample = 1;

    loop {
        let uptime_ms = monotonic_ms();
        print!("[heartbeat] tid={} uptime_ms={}\n", tid, uptime_ms);
        if uptime_ms >= next_dump_ms && sample <= 10 {
            dump_net_rx_counters(sample);
            sample += 1;
            next_dump_ms += 10_000;
        }
        let _ = sleep_ms(1_000);
    }
}
