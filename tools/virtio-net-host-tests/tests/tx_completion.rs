//! Transmit completion without receive traffic (virtio 1.2 sections 2.7.7 and
//! 5.1.6.2.3). With EVENT_IDX the driver takes completions back on its next
//! transmission and asks for no used-buffer interrupt while the queue has
//! room. When the queue fills it asks for one, so the completions the device
//! makes later interrupt, the interrupt raises the network softirq, and the
//! softirq's reclaim makes the queue available again.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::tcp_frame;

fn tx_interrupts() -> u32 {
    device::with(|d| d.queues[1].interrupts)
}

#[test]
fn a_full_transmit_queue_becomes_available_without_receive_traffic() {
    device::install(Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX | f::ANY_LAYOUT));
    net_mmio::init().expect("init");
    let frame = tcp_frame(40000, 22, 1000);

    // Completed at once: reclaimed by later transmissions, no interrupt each.
    for _ in 0..10 {
        net_mmio::transmit(&frame).expect("transmit");
    }
    assert_eq!(tx_interrupts(), 0, "completions with room left interrupted");

    // Now the device holds what it is given, and the ring fills.
    device::with(|d| d.hold_tx = true);
    let mut sent = 0;
    while net_mmio::transmit(&frame).is_ok() {
        sent += 1;
        assert!(sent <= 256, "more transmissions than descriptors");
    }
    assert_eq!(sent, 256, "the whole 256-entry ring was used");

    // No receive traffic. The device finishes half; that interrupts.
    device::with(|d| d.complete_tx(1, 128));
    assert_eq!(tx_interrupts(), 1, "the full queue asked for a completion interrupt");
    let softirqs = device::with(|d| d.softirqs);
    net_mmio::handle_interrupt();
    assert_eq!(device::with(|d| d.softirqs), softirqs + 1, "the interrupt raised NetRx");
    device::with(|d| assert_eq!(d.int_status, 0, "the interrupt was acknowledged"));

    // The softirq's reclaim takes them back, and transmission resumes.
    assert_eq!(net_mmio::reclaim_tx_completed(), 128);
    device::with(|d| d.hold_tx = false);
    net_mmio::transmit(&frame).expect("transmit after reclaim");
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
