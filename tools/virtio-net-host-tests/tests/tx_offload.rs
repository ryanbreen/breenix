//! Checksum and TCPv4 segmentation offload headers (virtio 1.2 section
//! 5.1.6.2): an IPv4 TCP batch longer than one MSS goes as one GSO_TCPV4
//! request with NEEDS_CSUM, the header length, the MSS and the TCP checksum
//! offsets; one within the MSS needs the checksum only; and a frame the
//! offload cannot describe is refused, for the caller to send in software.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio::{self, TxOffload};
use virtio_net_host_tests::support::tcp_frame;

fn header_of(bytes: &[u8]) -> (u8, u8, u16, u16, u16, u16, u16) {
    let le = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    (bytes[0], bytes[1], le(2), le(4), le(6), le(8), le(10))
}

#[test]
fn offload_headers_carry_valid_offsets_lengths_and_mss() {
    device::install(Config::legacy(
        f::MAC | f::STATUS | f::MRG_RXBUF | f::EVENT_IDX | f::ANY_LAYOUT | f::CSUM | f::HOST_TSO4,
    ));
    net_mmio::init().expect("init");
    let limit = net_mmio::tso_max_frame().expect("TSO negotiated");
    assert!(limit >= 14 + 20 + 20 + 3 * 1448, "batch limit {limit}");

    // Three segments' worth: GSO_TCPV4, NEEDS_CSUM, 54 header bytes, MSS
    // 1448, checksum from byte 34 stored 16 bytes in, num_buffers 0.
    let batch = tcp_frame(40000, 22, 3 * 1448);
    net_mmio::transmit_offload(&batch, TxOffload::Tcp4 { mss: 1448 }).expect("batch");
    // One segment's worth: the checksum offload alone.
    let single = tcp_frame(40000, 22, 1000);
    net_mmio::transmit_offload(&single, TxOffload::Tcp4 { mss: 1448 }).expect("single");
    // An ordinary frame.
    net_mmio::transmit(&single).expect("plain");

    device::with(|d| {
        assert_eq!(d.tx.len(), 3);
        let b = &d.tx[0];
        assert_eq!(header_of(&b.bytes), (1, 1, 54, 1448, 34, 16, 0));
        assert_eq!(&b.bytes[12..], &batch[..]);
        assert!(b.descriptors.len() > 1, "a {}-byte batch over 2 KiB buffers", batch.len());
        let s = &d.tx[1];
        assert_eq!(header_of(&s.bytes), (1, 0, 54, 0, 34, 16, 0));
        assert_eq!(&s.bytes[12..], &single[..]);
        assert_eq!(header_of(&d.tx[2].bytes), (0, 0, 0, 0, 0, 0, 0));
    });

    // Not IPv4 TCP: refused, and the device gets 0 more transmissions.
    let mut arp = single.clone();
    arp[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    assert!(net_mmio::transmit_offload(&arp, TxOffload::Tcp4 { mss: 1448 }).is_err());
    let mut udp = single.clone();
    udp[23] = 17;
    assert!(net_mmio::transmit_offload(&udp, TxOffload::Tcp4 { mss: 1448 }).is_err());
    // A TCP header length that runs past the frame.
    let mut short = tcp_frame(40000, 22, 0);
    short[46] = 0xf0;
    assert!(net_mmio::transmit_offload(&short, TxOffload::Tcp4 { mss: 1448 }).is_err());
    // A batch the device cannot take in one request.
    assert!(net_mmio::transmit_offload(&tcp_frame(1, 2, limit), TxOffload::Tcp4 { mss: 1448 }).is_err());
    assert!(net_mmio::transmit_offload(&batch, TxOffload::Tcp4 { mss: 0 }).is_err());
    device::with(|d| {
        assert_eq!(d.tx.len(), 3);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    });
}
