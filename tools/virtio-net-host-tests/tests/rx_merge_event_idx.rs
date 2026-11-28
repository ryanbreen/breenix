//! Merged receive buffers and used-buffer notification suppression on a legacy
//! device offering MRG_RXBUF, EVENT_IDX and a 4000-byte MTU (virtio 1.2
//! sections 2.7.7, 5.1.4 and 5.1.6.4). A frame longer than one receive buffer
//! arrives over several; malformed completions are dropped without losing
//! the buffers they name; and the device interrupts once per drain while the
//! 16-bit indices wrap.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, frame_number, numbered_frame, tcp_frame};

fn header(num_buffers: u16) -> Vec<u8> {
    let mut h = vec![0u8; 12];
    h[10..12].copy_from_slice(&num_buffers.to_le_bytes());
    h
}

fn rx_interrupts() -> u32 {
    device::with(|d| d.queues[0].interrupts)
}

#[test]
fn merged_frames_malformed_merges_and_event_index_wrap() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::MRG_RXBUF | f::EVENT_IDX | f::ANY_LAYOUT | f::MTU);
    cfg.mtu = 4000;
    device::install(cfg);
    net_mmio::init().expect("init");
    device::with(|d| {
        assert!(d.negotiated(f::MRG_RXBUF) && d.negotiated(f::EVENT_IDX) && d.negotiated(f::MTU));
        assert_eq!(d.avail_pending(0), 256);
    });

    // A 3000-byte frame fills one buffer and spills into a second.
    let big = tcp_frame(40000, 22, 3000 - 54);
    device::with(|d| {
        let before = d.queues[0].used_idx;
        assert!(d.deliver(0, &big));
        assert_eq!(d.queues[0].used_idx.wrapping_sub(before), 2, "the frame took two buffers");
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![big.clone()]);
    device::with(|d| assert_eq!(d.avail_pending(0), 256, "both buffers were offered again"));

    // Five malformed completions, of which 0 reach the stack.
    device::with(|d| {
        // num_buffers of 0.
        d.deliver_raw(0, &[(None, 100, Some(header(0)))]);
        // A buffer before the last that the device did not fill.
        d.deliver_raw(0, &[(None, 1000, Some(header(2))), (None, 100, None)]);
        // A length beyond the buffer.
        d.deliver_raw(0, &[(None, 3000, Some(header(1)))]);
        // More buffers than the largest frame needs.
        d.deliver_raw(0, &[(None, 2048, Some(header(3)))]);
        // A descriptor id outside the ring.
        d.deliver_raw(0, &[(Some(9999), 100, Some(header(1)))]);
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), Vec::<Vec<u8>>::new(), "malformed completions were delivered");
    device::with(|d| {
        // The 255 buffers the driver got back are offered again; the one the device
        // returned under a bad id is still the device's.
        assert_eq!(d.avail_pending(0), 255);
        assert!(d.deliver(0, &big));
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![big.clone()], "a good frame after the malformed ones");

    // Three frames per drain, one interrupt per drain, across the wrap of
    // used.idx and avail.idx, with a merged frame now and then.
    let mut next = 0u32;
    for round in 0..22_000u32 {
        let before = rx_interrupts();
        device::with(|d| {
            for i in 0..3 {
                assert!(d.deliver(0, &numbered_frame(next + i)), "round {round}: room");
            }
            if round % 1000 == 999 {
                assert!(d.deliver(0, &big));
            }
        });
        assert_eq!(rx_interrupts() - before, 1, "round {round}: interrupts");
        net_mmio::handle_interrupt();
        let frames = drain();
        let numbered: Vec<u32> = frames.iter().take(3).map(|fr| frame_number(fr)).collect();
        assert_eq!(numbered, vec![next, next + 1, next + 2], "round {round}");
        if round % 1000 == 999 {
            assert_eq!(frames.len(), 4);
            assert_eq!(frames[3], big);
        } else {
            assert_eq!(frames.len(), 3);
        }
        next += 3;
    }
    device::with(|d| {
        assert!(d.queues[0].used_idx < 30_000, "used.idx wrapped: {}", d.queues[0].used_idx);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    });
}
