//! The production setup on a legacy (version 1) virtio-mmio device that offers
//! a 256-entry queue maximum and the features this driver uses (virtio 1.2
//! sections 2.7.2, 4.2.4, 5.1.3 and 5.1.6).
use virtio_net_host_tests::device::{self, f, reg, status, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

/// What the driver uses, plus features it has no path for.
const OFFERED: u64 = f::MAC
    | f::STATUS
    | f::MRG_RXBUF
    | f::CSUM
    | f::HOST_TSO4
    | f::EVENT_IDX
    | f::ANY_LAYOUT
    | f::CTRL_VQ
    | f::MQ
    | f::GUEST_CSUM
    | f::GUEST_TSO4
    | f::CTRL_RX
    | f::NOTIFY_ON_EMPTY
    | f::INDIRECT_DESC;

/// The legacy feature word the driver accepts: MAC and STATUS, merged receive
/// buffers, checksum and TCPv4 segmentation offload, event indices, any
/// layout, and the control queue with multiqueue.
const ACCEPTED: u32 = (f::MAC
    | f::STATUS
    | f::MRG_RXBUF
    | f::CSUM
    | f::HOST_TSO4
    | f::EVENT_IDX
    | f::ANY_LAYOUT
    | f::CTRL_VQ
    | f::MQ) as u32;

#[test]
fn legacy_setup_uses_the_devices_256_entries_and_negotiates_the_legacy_word() {
    device::install(Config::legacy(OFFERED));
    net_mmio::init().expect("init");

    device::with(|d| {
        // Section 4.2.4: the guest page size is written after the reset and
        // before any queue is set up.
        let reset = d.log.iter().position(|w| w.offset == reg::STATUS && w.value == 0);
        let page = d.first_write(reg::GUEST_PAGE_SIZE);
        let first_queue = d.first_write(reg::QUEUE_SEL);
        assert!(reset.is_some(), "the device was reset");
        assert_eq!(d.writes_to(reg::GUEST_PAGE_SIZE).last().map(|w| w.value), Some(4096));
        assert!(page > reset, "GuestPageSize at {page:?} precedes the reset at {reset:?}");
        assert!(page < first_queue, "GuestPageSize at {page:?} follows queue setup at {first_queue:?}");

        // The legacy interface has one feature word and no FEATURES_OK.
        let words: Vec<_> = d.writes_to(reg::DRIVER_FEATURES).iter().map(|w| w.value).collect();
        assert_eq!(words.first().copied(), Some(ACCEPTED), "accepted {words:#x?}");
        assert!(words[1..].iter().all(|&w| w == 0), "high words {words:#x?}");
        assert!(
            d.status_writes.iter().all(|s| s & status::FEATURES_OK == 0),
            "FEATURES_OK written to a legacy device: {:?}",
            d.status_writes
        );

        // Both data queues take the device's 256 entries, page-aligned, from
        // the page their PFN names, with the used ring at the next page
        // boundary after the 4096 + 518 bytes before it (section 2.7.2).
        for q in [0u32, 1] {
            let queue = &d.queues[q as usize];
            assert_eq!(queue.num, 256, "queue {q} size");
            assert_eq!(queue.align, 4096, "queue {q} alignment");
            assert_ne!(queue.pfn, 0, "queue {q} PFN");
            assert_eq!(queue.desc, u64::from(queue.pfn) * 4096, "queue {q} base");
            assert_eq!(queue.used - queue.desc, 8192, "queue {q} used ring offset");
            assert_eq!(unsafe { core::ptr::read_volatile((queue.used + 2) as *const u16) }, 0);
        }
        // DRIVER_OK comes after the queues are configured.
        let last_pfn = d.log.iter().rposition(|w| w.offset == reg::QUEUE_PFN);
        let driver_ok = d
            .log
            .iter()
            .position(|w| w.offset == reg::STATUS && w.value & status::DRIVER_OK != 0);
        assert!(driver_ok > last_pfn, "DRIVER_OK at {driver_ok:?} before queue PFN at {last_pfn:?}");

        // Receive capacity is posted for the whole ring, not four buffers.
        let posted = d.avail_pending(0);
        assert!(posted > 4, "{posted} receive buffers posted");
        assert_eq!(posted, 256);
    });

    // With merged receive buffers the legacy header is 12 bytes, both ways.
    let frame = tcp_frame(40000, 22, 100);
    net_mmio::transmit(&frame).expect("transmit");
    device::with(|d| {
        let sent = d.tx.last().expect("one transmission").bytes.clone();
        assert_eq!(sent.len(), 12 + frame.len());
        assert_eq!(&sent[..12], &[0u8; 12], "flags, GSO, offsets and num_buffers are zero");
        assert_eq!(&sent[12..], &frame[..]);
        assert!(d.deliver(0, &frame), "the driver offered room");
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![frame]);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
