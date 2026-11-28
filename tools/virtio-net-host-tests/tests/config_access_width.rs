//! Configuration fields are read at their own width (virtio 1.2 section
//! 4.2.2.2): on a modern device offering MTU and MQ, `max_virtqueue_pairs`
//! (offset 8) and `mtu` (offset 10) are 16-bit fields, so each is read with
//! one aligned 16-bit access, and the 8-bit `mac` bytes with 8-bit accesses.
use virtio_net_host_tests::device::{self, f, Config};
use virtio_net_host_tests::drivers::virtio::net_mmio;

#[test]
fn sixteen_bit_configuration_fields_are_read_sixteen_bits_wide() {
    let mut cfg = Config::legacy(f::VERSION_1 | f::MAC | f::STATUS | f::MTU | f::CTRL_VQ | f::MQ);
    cfg.version = 2;
    cfg.max_pairs = 2;
    cfg.cpus = 2;
    cfg.mtu = 1500;
    device::install(cfg);
    net_mmio::init().expect("init");

    device::with(|d| {
        let reads = &d.config_reads;
        for field in [8u64, 10] {
            assert!(
                reads.contains(&(field, 2)),
                "the 16-bit field at {field} was not read 16 bits wide: {reads:?}"
            );
        }
        for &(at, width) in reads {
            let ok = match at {
                0..=5 => width == 1,
                8 | 10 => width == 2,
                _ => false,
            };
            assert!(ok, "configuration read at {at}, {width} bytes wide: {reads:?}");
        }
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    });
}
