//! Queue pairs before queue depth (virtio 1.2 section 5.1.6.5.6). A device
//! with CTRL_VQ, MQ, four pairs and 1024-entry queues, on four CPUs: one pair
//! at 1024 entries takes over half the driver's 2048-page DMA budget, so four
//! pairs fit only at a smaller depth. The driver asks for four pairs, and each
//! receive queue delivers.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn deep_queues_still_give_one_pair_per_cpu() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX | f::ANY_LAYOUT | f::CTRL_VQ | f::MQ);
    cfg.queue_max = 1024;
    cfg.max_pairs = 4;
    cfg.cpus = 4;
    device::install(cfg);
    net_mmio::init().expect("init");

    device::with(|d| {
        assert_eq!(d.ctrl, vec![vec![4, 0, 4, 0]], "four pairs for four CPUs");
        for rx in [0u32, 2, 4, 6] {
            let num = d.queues[rx as usize].num;
            assert!((16..=1024).contains(&num) && num.is_power_of_two(), "receiveq {rx} size {num}");
            assert_eq!(u32::from(d.avail_pending(rx)), num, "receiveq {rx} posted");
            assert!(d.deliver(rx, &tcp_frame(22, 40000 + rx as u16, 64)));
        }
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain().len(), 4);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
