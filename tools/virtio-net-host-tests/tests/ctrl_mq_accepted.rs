//! Multiqueue that the device accepts (virtio 1.2 section 5.1.6.5.6): four
//! pairs for four CPUs, a flow keeps to one transmit queue, flows spread over
//! the pairs, and each of the 4 receive queues delivers.
use std::collections::BTreeMap;
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;
use virtio_net_host_tests::support::{drain, tcp_frame};

#[test]
fn accepted_queue_pairs_carry_stable_flows() {
    let mut cfg = Config::legacy(f::MAC | f::STATUS | f::EVENT_IDX | f::ANY_LAYOUT | f::CTRL_VQ | f::MQ);
    cfg.max_pairs = 8;
    cfg.cpus = 4;
    device::install(cfg);
    net_mmio::init().expect("init");
    device::with(|d| assert_eq!(d.ctrl, vec![vec![4, 0, 4, 0]], "four pairs for four CPUs"));

    let mut queue_of = BTreeMap::new();
    for round in 0..3 {
        for port in 0..32u16 {
            net_mmio::transmit(&tcp_frame(40000 + port, 22, 64)).expect("transmit");
            let q = device::with(|d| d.tx.last().unwrap().queue);
            assert!(q % 2 == 1 && q < 8, "transmit queue {q}");
            let first = *queue_of.entry(port).or_insert(q);
            assert_eq!(first, q, "round {round}: flow {port} moved");
        }
    }
    let used: std::collections::BTreeSet<_> = queue_of.values().copied().collect();
    assert!(used.len() > 1, "32 flows all used queue {used:?}");

    device::with(|d| {
        for rx in [0u32, 2, 4, 6] {
            assert_eq!(d.avail_pending(rx), 256, "receiveq {rx} posted");
            assert!(d.deliver(rx, &tcp_frame(22, 40000, 64)));
        }
    });
    net_mmio::handle_interrupt();
    assert_eq!(drain().len(), 4);
    device::with(|d| assert!(d.errors.is_empty(), "{:?}", d.errors));
}
