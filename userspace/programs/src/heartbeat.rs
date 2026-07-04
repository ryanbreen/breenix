//! Minimal non-GPU userspace heartbeat for scheduler liveness diagnosis.

use libbreenix::fs::{self, O_RDONLY};
use libbreenix::io;
use libbreenix::process::gettid;
use libbreenix::time::{now_monotonic, sleep_ms};

const COUNTER_PATH: &str = "/proc/trace/counters";
const XHCI_COUNTER_PATH: &str = "/proc/xhci/counters";
const NET_RX_COUNTER_PREFIXES: &[&str] = &[
    "NET_RX_MSI_TOTAL:",
    "NET_RX_RING_DRAIN_TOTAL:",
    "NET_RX_FRAME_TOTAL:",
    "NET_RX_ARP_TOTAL:",
    "NET_RX_ETHERTYPE_OTHER_TOTAL:",
    "NET_RX_SOFTIRQ_ENTRY_TOTAL:",
    "NET_RX_SOFTIRQ_EXIT_TOTAL:",
    "NET_RX_REENTRANT_SKIP_TOTAL:",
    "NET_RX_GUARD_RELEASE_TOTAL:",
    "NET_RX_REARM_CHECK_TOTAL:",
    "NET_RX_REARM_RACE_TOTAL:",
    "NET_RX_REARM_ARMED_TOTAL:",
    "NET_PCI_IRQ_RAISED_NETRX:",
    "NET_PCI_DEVICE_STATUS:",
    "NET_PCI_ISR_STATUS:",
    "NET_PCI_DEVICE_FEATURES:",
    "NET_PCI_GUEST_FEATURES:",
    "NET_PCI_RX_QUEUE_PFN:",
    "NET_PCI_TX_QUEUE_PFN:",
    "NET_PCI_RX_QUEUE_SIZE:",
    "NET_PCI_TX_QUEUE_SIZE:",
    "NET_PCI_RX_QUEUE_ALIGN:",
    "NET_PCI_RX_QUEUE_VECTOR:",
    "NET_PCI_TX_QUEUE_VECTOR:",
    "NET_PCI_RX_AVAIL_FLAGS:",
    "NET_PCI_RX_AVAIL_IDX:",
    "NET_PCI_RX_USED_FLAGS:",
    "NET_PCI_RX_USED_IDX:",
    "NET_PCI_RX_LAST_USED_IDX:",
    "NET_PCI_RX_POSTED_GAP:",
    "NET_PCI_RX_DESC0:",
    "NET_PCI_RX_DESC1:",
    "NET_PCI_RX_DESC2:",
    "NET_PCI_RX_DESC3:",
    "NET_PCI_RX_RING_HEADS:",
    "NET_RX_PROCESSING_HELD:",
    "NET_RX_PENDING_WHILE_PROCESSING:",
    "NET_PCI_RX_REARM_SAMPLE_SEQ:",
    "NET_PCI_RX_REARM_SAMPLE0:",
    "NET_PCI_RX_REARM_SAMPLE1:",
    "NET_PCI_RX_REARM_SAMPLE2:",
    "NET_PCI_RX_REARM_SAMPLE3:",
    "NET_PCI_RX_SOFTIRQ_ENTRY_SAMPLE:",
    "NET_PCI_RX_SOFTIRQ_EXIT_SAMPLE:",
    "GIC_SPI55_ACK_TOTAL:",
    "GICV2M_BASE_PHYS:",
    "GICV2M_DOORBELL_PHYS:",
    "GICV2M_MSI_TYPER:",
    "GICV2M_SPI_BASE:",
    "GICV2M_SPI_COUNT:",
    "GICV2M_NEXT_INDEX:",
    "GIC_SPI54_IRQ:",
    "GIC_SPI54_VERSION:",
    "GIC_SPI54_ISENABLER_BIT:",
    "GIC_SPI54_ISPENDR_BIT:",
    "GIC_SPI54_ISACTIVER_BIT:",
    "GIC_SPI54_IGROUPR_BIT:",
    "GIC_SPI54_PRIORITY:",
    "GIC_SPI54_ICFGR_REG:",
    "GIC_SPI54_IROUTER:",
    "GIC_SPI54_ITARGETSR_BYTE:",
    "GIC_SPI54_GICD_CTLR:",
    "GIC_SPI55_IRQ:",
    "GIC_SPI55_VERSION:",
    "GIC_SPI55_ISENABLER_BIT:",
    "GIC_SPI55_ISPENDR_BIT:",
    "GIC_SPI55_ISACTIVER_BIT:",
    "GIC_SPI55_IGROUPR_BIT:",
    "GIC_SPI55_PRIORITY:",
    "GIC_SPI55_ICFGR_REG:",
    "GIC_SPI55_IROUTER:",
    "GIC_SPI55_ITARGETSR_BYTE:",
    "GIC_SPI55_GICD_CTLR:",
    "MANUAL_GICV2M_TEST_DONE:",
    "MANUAL_GICV2M_TEST_IRQ:",
    "MANUAL_GICV2M_TEST_DOORBELL:",
    "MANUAL_GICV2M_TEST_BEFORE_PEND:",
    "MANUAL_GICV2M_TEST_AFTER_WRITE_PEND:",
    "MANUAL_GICV2M_TEST_AFTER_WAIT_PEND:",
    "MANUAL_GICV2M_TEST_ACK_BEFORE:",
    "MANUAL_GICV2M_TEST_ACK_AFTER:",
    "MANUAL_GICV2M_TEST_MSI_BEFORE:",
    "MANUAL_GICV2M_TEST_MSI_AFTER:",
];

/// Read the KBD_NONZERO_TOTAL counter from /proc/xhci/counters.
///
/// This is a guest-observable, monotonic count of non-empty USB-HID
/// keyboard reports (kernel/src/drivers/usb/hid.rs::NONZERO_KBD_COUNT). The
/// Parallels launcher-smoke test harness's keyboard-delivery handshake
/// baselines this value, injects a single inert probe key, and polls for it
/// to increase -- proving the injected key actually reached the guest's HID
/// stack before running the real (timing-sensitive) test gesture. Printed
/// every heartbeat tick (~1/s) so the handshake's poll latency is bounded by
/// this cadence, not by the 10s/20s dump_net_rx_counters() schedule.
fn read_kbd_nonzero_total() -> Option<u64> {
    let fd = fs::open(XHCI_COUNTER_PATH, O_RDONLY).ok()?;
    let mut buf = [0u8; 512];
    let n = io::read(fd, &mut buf).ok()?;
    let _ = io::close(fd);
    let text = core::str::from_utf8(&buf[..n]).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("KBD_NONZERO_TOTAL=") {
            return rest.trim().parse::<u64>().ok();
        }
    }
    None
}

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

    let mut buf = [0u8; 16 * 1024];
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
        let kbd_nonzero = read_kbd_nonzero_total().unwrap_or(0);
        print!(
            "[heartbeat] tid={} uptime_ms={} kbd_nonzero={}\n",
            tid, uptime_ms, kbd_nonzero
        );
        if uptime_ms >= next_dump_ms && sample <= 10 {
            dump_net_rx_counters(sample);
            sample += 1;
            next_dump_ms += 10_000;
        }
        let _ = sleep_ms(1_000);
    }
}
