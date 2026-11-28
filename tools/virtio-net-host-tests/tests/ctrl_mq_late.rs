//! A VQ_PAIRS_SET answered after the driver stopped waiting (virtio 1.2
//! section 5.1.6.5.6). The device takes the command and answers
//! VIRTIO_NET_OK only once `init` has given up on it. From then on the device
//! may steer received frames to the second receive queue, so the driver must
//! still drain it; it transmits on the first pair, since it never saw the
//! device accept more.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn a_late_queue_pair_answer_does_not_strand_received_frames() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX | f::ANY_LAYOUT | f::CTRL_VQ | f::MQ);
    cfg.max_pairs = 2;
    cfg.cpus = 2;
    cfg.ctrl_late = true;
    device::install(cfg);
    net_mmio::init().expect("init");
    device::with(|d| {
        assert_eq!(d.ctrl, vec![vec![4, 0, 2, 0]], "two pairs asked for");
        assert_eq!(d.ctrl_unanswered.len(), 1, "the command is still the device's");
        d.answer_ctrl();
    });

    // Received on each pair's receive queue after the late acceptance.
    device::with(|d| {
        for rx in [0u32, 2] {
            assert_eq!(d.avail_pending(rx), 256, "receiveq {rx} posted");
            assert!(d.deliver(rx, &tcp_frame(22, 40000 + rx as u16, 64)), "receiveq {rx}");
        }
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain().len(), 2, "a frame on the second receive queue was not drained");

    // Unconfirmed pairs carry no transmissions.
    for port in 0..16u16 {
        net_mmio::transmit(&tcp_frame(40000 + port, 22, 64)).expect("transmit");
    }
    device::with(|d| {
        assert!(d.tx.iter().all(|p| p.queue == 1), "queues {:?}", d.tx.iter().map(|p| p.queue).collect::<Vec<_>>());
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    });
}
