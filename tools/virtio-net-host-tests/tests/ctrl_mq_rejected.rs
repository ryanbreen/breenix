//! Multiqueue that the device refuses (virtio 1.2 sections 5.1.5 and 5.1.6.5.6):
//! with CTRL_VQ and MQ offered and four queue pairs available, the driver
//! configures the pairs it wants, asks for them with VQ_PAIRS_SET, and, when
//! the device answers VIRTIO_NET_ERR, keeps to the first pair.
use virtio_net_host_tests::device::{self, f, reg, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn a_rejected_queue_pair_count_keeps_one_pair() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX | f::ANY_LAYOUT | f::CTRL_VQ | f::MQ);
    cfg.max_pairs = 4;
    cfg.ctrl_ack = 1; // VIRTIO_NET_ERR
    device::install(cfg);
    net_mmio::init().expect("init");

    device::with(|d| {
        // Class VIRTIO_NET_CTRL_MQ, command VQ_PAIRS_SET, four pairs (le16).
        assert_eq!(d.ctrl, vec![vec![4, 0, 4, 0]]);
        // The extra queues were configured before the command was offered.
        let notify = d.log.iter().position(|w| w.offset == reg::QUEUE_NOTIFY && w.value == 8);
        for q in 2..8u32 {
            let pfn = d.log.iter().position(|w| w.offset == reg::QUEUE_PFN && w.queue == q && w.value != 0);
            assert!(pfn.is_some() && pfn < notify, "queue {q} configured at {pfn:?}, command at {notify:?}");
        }
    });

    // The 16 flows each transmit on transmitq1, and receiveq1 still receives.
    for port in 0..16u16 {
        net_mmio::transmit(&tcp_frame(40000 + port, 22, 64)).expect("transmit");
    }
    device::with(|d| {
        assert_eq!(d.tx.len(), 16);
        assert!(d.tx.iter().all(|p| p.queue == 1), "queues {:?}", d.tx.iter().map(|p| p.queue).collect::<Vec<_>>());
        assert!(d.deliver(0, &tcp_frame(22, 40000, 64)));
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain().len(), 1);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
