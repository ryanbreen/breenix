//! The virtio-net PCI driver holds transmissions submitted while it is
//! consuming its RX ring and publishes them, in order, once the RX buffers
//! are re-posted (`kernel/src/drivers/virtio/tx_hold.rs`). That rule used to
//! live in TCP as a queue for SYN+ACKs only (audit locals, DEV-29(4)); it is
//! a property of one device, so it belongs to that device's driver.
//!
//! Parallels' device drops a transmission published while its RX ring is
//! being consumed, so what matters is the order of the device operations:
//! no TX publish or TX notify between a packet being handed up and the RX
//! re-post and RX notify, then the held slots in order and one TX notify.
//! The driver hands those operations to the hold through `TxRings`, so the
//! checks below record exactly the sequence `net_pci.rs` performs, and the
//! last check reads `net_pci.rs` to see that it does go through the hold.

#[path = "../kernel/src/drivers/virtio/tx_hold.rs"]
mod tx_hold;

use std::fs;
use std::path::PathBuf;
use tx_hold::{TxHold, TxRings};

#[derive(Debug, PartialEq)]
enum Op {
    PublishTx(u16),
    NotifyTx,
    RepostRx,
    NotifyRx,
}

#[derive(Default)]
struct Recorder(Vec<Op>);

impl TxRings for Recorder {
    fn publish_tx(&mut self, slot: u16) {
        self.0.push(Op::PublishTx(slot));
    }
    fn notify_tx(&mut self) {
        self.0.push(Op::NotifyTx);
    }
    fn repost_rx(&mut self) {
        self.0.push(Op::RepostRx);
    }
    fn notify_rx(&mut self) {
        self.0.push(Op::NotifyRx);
    }
}

#[test]
fn a_transmission_outside_rx_consumption_is_published_and_notified_at_once() {
    let mut hold = TxHold::<4>::new();
    let mut rings = Recorder::default();
    hold.transmit(&mut rings, 3);
    assert_eq!(rings.0, [Op::PublishTx(3), Op::NotifyTx]);
}

#[test]
fn rx_is_reposted_and_notified_before_any_held_transmission_is_published() {
    let mut hold = TxHold::<4>::new();
    let mut rings = Recorder::default();
    hold.begin_rx();
    hold.transmit(&mut rings, 2);
    hold.begin_rx(); // a second packet of the same batch
    hold.transmit(&mut rings, 0);
    assert!(rings.0.is_empty(), "nothing reaches the TX ring while RX is being consumed: {:?}", rings.0);

    hold.finish_rx(&mut rings);
    assert_eq!(
        rings.0,
        [Op::RepostRx, Op::NotifyRx, Op::PublishTx(2), Op::PublishTx(0), Op::NotifyTx]
    );

    // Consumption is over: the next transmission goes at once.
    rings.0.clear();
    hold.transmit(&mut rings, 1);
    assert_eq!(rings.0, [Op::PublishTx(1), Op::NotifyTx]);
}

#[test]
fn finishing_rx_with_nothing_held_touches_only_the_rx_ring() {
    let mut hold = TxHold::<4>::new();
    let mut rings = Recorder::default();
    hold.begin_rx();
    hold.finish_rx(&mut rings);
    assert_eq!(rings.0, [Op::RepostRx, Op::NotifyRx]);
}

#[test]
fn a_full_pool_of_held_slots_fits() {
    // Each held entry owns a claimed TX slot, so the hold needs no more
    // room than the pool has slots.
    let mut hold = TxHold::<4>::new();
    let mut rings = Recorder::default();
    hold.begin_rx();
    for slot in 0..4 {
        hold.transmit(&mut rings, slot);
    }
    hold.finish_rx(&mut rings);
    assert_eq!(
        rings.0,
        [
            Op::RepostRx,
            Op::NotifyRx,
            Op::PublishTx(0),
            Op::PublishTx(1),
            Op::PublishTx(2),
            Op::PublishTx(3),
            Op::NotifyTx,
        ]
    );
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap_or_else(|| panic!("find {signature}"));
    let open = start + source[start..].find('{').unwrap_or_else(|| panic!("open brace of {signature}"));
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("close brace of {signature}")
}

/// The sequences above are only the driver's if the driver uses them: its
/// transmit and RX recycle go through the hold, and neither publishes,
/// re-posts or notifies on its own; the ring operations the hold orders are
/// the driver's real ring and doorbell writes.
#[test]
fn the_pci_driver_orders_its_rings_through_the_hold() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("kernel/src/drivers/virtio/net_pci.rs"))
        .expect("read net_pci.rs");
    let code: String = source
        .lines()
        .map(|line| line.find("//").map_or(line, |cut| &line[..cut]))
        .collect::<Vec<_>>()
        .join("\n");

    let mut findings = Vec::new();
    let transmit = function_body(&code, "pub fn transmit(");
    if !transmit.contains(".transmit(") {
        findings.push("transmit does not hand its slot to the hold");
    }
    let recycle = function_body(&code, "pub fn recycle_rx_buffers(");
    if !recycle.contains(".finish_rx(") {
        findings.push("recycle_rx_buffers does not end RX consumption through the hold");
    }
    for (name, body) in [("transmit", transmit), ("recycle_rx_buffers", recycle)] {
        for direct in ["publish_tx_slot(", "REG_QUEUE_NOTIFY", "avail.idx", "avail.ring"] {
            if body.contains(direct) {
                findings.push(if name == "transmit" {
                    "transmit touches a ring or doorbell itself"
                } else {
                    "recycle_rx_buffers touches a ring or doorbell itself"
                });
                break;
            }
        }
    }
    if !function_body(&code, "pub fn receive(").contains(".begin_rx()") {
        findings.push("receive does not open RX consumption when it hands a packet up");
    }

    let rings = function_body(&code, "impl tx_hold::TxRings for");
    let op = |name: &str| function_body(rings, &format!("fn {name}("));
    if !op("publish_tx").contains("publish_tx_slot(") {
        findings.push("publish_tx is not the TX avail-ring publish");
    }
    if !op("notify_tx").contains("REG_QUEUE_NOTIFY, 1") {
        findings.push("notify_tx does not ring the TX queue's doorbell");
    }
    if !op("repost_rx").contains("PCI_RX_QUEUE") || !op("repost_rx").contains("avail.idx") {
        findings.push("repost_rx does not offer the buffers back on the RX avail ring");
    }
    if !op("notify_rx").contains("REG_QUEUE_NOTIFY, 0") {
        findings.push("notify_rx does not ring the RX queue's doorbell");
    }
    assert!(findings.is_empty(), "net_pci.rs bypasses the TX hold: {findings:?}");
}
