//! An accepted MTU fits the receive ring (virtio 1.2 section 5.1.6.3.1: with
//! VIRTIO_NET_F_MTU the driver supplies enough receive buffers for one packet
//! of the MTU plus the Ethernet header). The device offers MRG_RXBUF and a
//! 65535-byte MTU on 32-entry queues: 32 buffers of 2 KiB hold 65536 bytes,
//! short of the header and frame. Whatever the driver accepts, the largest
//! frame it accepted must arrive and reach the stack whole.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn the_largest_accepted_frame_fits_the_posted_receive_buffers() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::MRG_RXBUF | f::ANY_LAYOUT | f::MTU);
    cfg.mtu = 65535;
    cfg.queue_max = 32;
    device::install(cfg);
    net_mmio::init().expect("init");

    let (mtu, posted) = device::with(|d| (d.negotiated(f::MTU), d.avail_pending(0)));
    assert_eq!(posted, 32, "the 32-entry receive queue is posted");
    // The largest frame the driver accepted: the MTU's, or 1514 without it.
    let ip_len = if mtu { 65535 } else { 1500 };
    let frame = tcp_frame(22, 40000, ip_len - 40);
    device::with(|d| {
        assert!(
            d.deliver(0, &frame),
            "MTU negotiated {mtu}: a {}-byte frame does not fit {posted} posted buffers",
            frame.len()
        );
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![frame]);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
