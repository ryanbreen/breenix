//! Run with `rustc --edition=2021 --test tests/procfs_receive_counters_host.rs`.
extern crate alloc;

#[path = "../kernel/src/fs/attrs.rs"]
pub mod attrs;
pub mod fs {
    pub use crate::attrs;
}

pub mod net {
    pub mod dhcp {
        pub struct Lease {
            pub nameservers: [Option<[u8; 4]>; 3],
            pub ntp_servers: [Option<[u8; 4]>; 3],
            pub server: [u8; 4],
        }

        pub fn lease() -> Option<Lease> {
            None
        }

        pub fn configuration_required() -> bool {
            false
        }
    }

    pub mod tcp {
        pub struct Counters {
            pub checksum_rejected: u64,
            pub outside_window: u64,
            pub gap_segments: u64,
            pub ack_output_failed: u64,
        }

        pub fn receive_counters() -> Counters {
            Counters {
                checksum_rejected: 0,
                outside_window: 0,
                gap_segments: 42,
                ack_output_failed: 0,
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub mod drivers {
    pub mod bcm5720 {
        pub struct Counters {
            pub msi: u64,
            pub no_rx_bd: u64,
            pub in_discards: u64,
            pub in_errors: u64,
            pub bad_completions: u64,
            pub tx_ring_full: u64,
        }

        pub fn receive_counters() -> Option<Counters> {
            None
        }
    }
}

#[path = "../kernel/src/fs/procfs/net.rs"]
pub mod procfs_net;

// The kernel uses this sibling generator through the procfs dispatcher.
const _: fn() -> Result<String, i32> = procfs_net::generate_pnp;

#[test]
fn receive_counters_are_read_only_for_root_and_wheel() {
    let attrs = procfs_net::receive_counters_attrs("/proc/net/rx_counters").unwrap();
    assert_eq!((attrs.mode, attrs.uid, attrs.gid), (0o100440, 0, 10));
    assert!(procfs_net::list_entries()
        .unwrap()
        .iter()
        .any(|name| name == "rx_counters"));
    assert!(procfs_net::generate_receive_counters()
        .unwrap()
        .contains("tcp_rx_gap_segments: 42\n"));
}
