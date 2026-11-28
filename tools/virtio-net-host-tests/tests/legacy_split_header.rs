//! A legacy device that offers neither merged receive buffers nor
//! VIRTIO_F_ANY_LAYOUT: the header is 10 bytes, and it sits in a descriptor of
//! its own on both transmit and receive (virtio 1.2 sections 5.1.6.1 and 5.1.6.6).
use virtio_net_host_tests::device::{self, f, reg, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn without_any_layout_the_ten_byte_header_has_its_own_descriptor() {
    device::install(Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX));
    net_mmio::init().expect("init");

    let frame = tcp_frame(40000, 22, 300);
    net_mmio::transmit(&frame).expect("transmit");
    device::with(|d| {
        let accepted = d.writes_to(reg::DRIVER_FEATURES).first().map(|w| w.value);
        assert_eq!(accepted, Some((f::MAC | f::STATUS | f::EVENT_IDX) as u32));

        let sent = d.tx.last().expect("one transmission").clone();
        assert_eq!(sent.descriptors[0].1, 10, "header descriptor {:?}", sent.descriptors);
        assert!(sent.descriptors.len() >= 2);
        assert_eq!(&sent.bytes[..10], &[0u8; 10]);
        assert_eq!(&sent.bytes[10..], &frame[..]);

        // Each receive buffer is a header descriptor and a data descriptor.
        let posted = d.avail_pending(0);
        assert!(posted > 4, "{posted} receive buffers posted");
        let saved = d.queues[0].last_avail;
        let head = d.take_avail(0).expect("a receive buffer");
        let chain = d.chain(0, head);
        d.queues[0].last_avail = saved;
        assert_eq!(chain.len(), 2, "receive chain {chain:?}");
        assert_eq!(chain[0].1, 10);
        assert!(chain[1].1 >= 1514);

        assert!(d.deliver(0, &frame));
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![frame]);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
