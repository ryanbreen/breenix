//! The production setup on a modern (version 2) virtio-mmio device: VERSION_1
//! is accepted and FEATURES_OK confirmed before the queues are given by
//! address and made ready (virtio 1.2 sections 3.1.1 and 4.2.3).
use virtio_net_host_tests::device::{self, f, reg, status, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn modern_setup_accepts_version_1_and_confirms_features_ok() {
    let mut cfg = Config::legacy(
        f::VERSION_1 | f::MAC | f::STATUS | f::MRG_RXBUF | f::EVENT_IDX | f::CSUM | f::HOST_TSO4,
    );
    cfg.version = 2;
    device::install(cfg);
    net_mmio::init().expect("init");

    device::with(|d| {
        let words: Vec<_> = d.writes_to(reg::DRIVER_FEATURES).iter().map(|w| w.value).collect();
        assert_eq!(
            words,
            vec![(f::MAC | f::STATUS | f::MRG_RXBUF | f::EVENT_IDX | f::CSUM | f::HOST_TSO4) as u32, 1],
            "both feature words, VERSION_1 in the high one"
        );
        // ACKNOWLEDGE, DRIVER, FEATURES_OK (kept by the device), then DRIVER_OK.
        let ok = d.status_writes.iter().position(|s| s & status::FEATURES_OK != 0);
        let driver_ok = d.status_writes.iter().position(|s| s & status::DRIVER_OK != 0);
        assert!(ok.is_some(), "FEATURES_OK written: {:?}", d.status_writes);
        assert!(ok < driver_ok, "status writes {:?}", d.status_writes);
        assert_ne!(d.status & status::FEATURES_OK, 0);
        assert!(d.writes_to(reg::GUEST_PAGE_SIZE).is_empty());
        assert!(d.writes_to(reg::QUEUE_PFN).is_empty());

        let last_ready = d.log.iter().rposition(|w| w.offset == reg::QUEUE_READY && w.value == 1);
        let driver_ok_at = d
            .log
            .iter()
            .position(|w| w.offset == reg::STATUS && w.value & status::DRIVER_OK != 0);
        assert!(driver_ok_at > last_ready);
        for q in [0u32, 1] {
            let queue = &d.queues[q as usize];
            assert!(queue.ready, "queue {q} ready");
            assert_eq!(queue.num, 256, "queue {q} size");
            assert_eq!(queue.desc % 16, 0);
            assert_eq!(queue.avail, queue.desc + 16 * 256, "queue {q} available ring");
            assert_eq!(queue.used % 4, 0);
            assert!(queue.used >= queue.avail + 6 + 2 * 256, "queue {q} used ring");
        }
        assert_eq!(d.avail_pending(0), 256);
    });

    // With VERSION_1 the header is 12 bytes (section 5.1.6).
    let frame = tcp_frame(40000, 22, 64);
    net_mmio::transmit(&frame).expect("transmit");
    device::with(|d| {
        let sent = &d.tx.last().expect("one transmission").bytes;
        assert_eq!(&sent[..12], &[0u8; 12]);
        assert_eq!(&sent[12..], &frame[..]);
        assert!(d.deliver(0, &frame));
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain(), vec![frame]);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
