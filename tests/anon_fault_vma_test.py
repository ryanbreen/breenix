"""Count VMA probes in the kernel's private anonymous first-touch lookup."""

from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]

FIXTURE = """
use std::cell::Cell;
use std::cmp::Ordering;

thread_local! { static PROBES: Cell<usize> = const { Cell::new(0) }; }

fn probe() {
    PROBES.with(|count| count.set(count.get() + 1));
}

mod memory {
    pub mod vma {
        use super::super::{probe, Ordering};

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct VirtAddr(u64);

        impl VirtAddr {
            pub fn new(value: u64) -> Self { Self(value) }
            pub fn as_u64(self) -> u64 {
                probe();
                self.0
            }
        }

        impl Ord for VirtAddr {
            fn cmp(&self, other: &Self) -> Ordering {
                self.as_u64().cmp(&other.0)
            }
        }

        impl PartialOrd for VirtAddr {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        pub struct Reservation(pub u64);

        impl PartialEq<u64> for Reservation {
            fn eq(&self, other: &u64) -> bool {
                probe();
                self.0 == *other
            }
        }

        #[derive(Clone, Copy)]
        pub struct MmapFlags(pub u8);

        impl MmapFlags {
            pub const ANONYMOUS: Self = Self(1);
            pub const PRIVATE: Self = Self(2);

            pub fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Protection { ReadWrite }

        pub struct Vma {
            pub start: VirtAddr,
            pub end: VirtAddr,
            pub reservation: Reservation,
            pub prot: Protection,
            pub flags: MmapFlags,
        }

        impl Vma {
            pub fn contains(&self, addr: VirtAddr) -> bool {
                self.start <= addr && addr < self.end
            }
        }
    }

    pub mod process_memory {
        use super::vma::VirtAddr;

        pub struct ProcessPageTable(pub Option<VirtAddr>);

        impl ProcessPageTable {
            pub fn translate(&self, addr: VirtAddr) -> Option<()> {
                (self.0 == Some(addr)).then_some(())
            }
        }
    }

    mod anon_map {
        use super::process_memory::ProcessPageTable;
        use super::vma::{MmapFlags, Protection, Reservation, VirtAddr, Vma};
        use crate::PROBES;

        LOOKUP_SOURCE

        fn lookup_probes(count: u64) -> [usize; 2] {
            const PAGE: u64 = 4096;
            let mut vmas: Vec<_> = (0..count).rev().map(|index| {
                let start = (index * 2 + 1) * PAGE;
                Vma {
                    start: VirtAddr::new(start),
                    end: VirtAddr::new(start + PAGE),
                    reservation: Reservation(0),
                    prot: Protection::ReadWrite,
                    flags: MmapFlags(MmapFlags::ANONYMOUS.0 | MmapFlags::PRIVATE.0),
                }
            }).collect();
            let table = ProcessPageTable(None);

            PROBES.with(|probes| probes.set(0));
            assert!(!reserved_and_absent(&table, &vmas, PAGE));
            let reservation = PROBES.with(|probes| probes.replace(0));
            assert_eq!(anonymous_protection(&vmas, PAGE), Some(Protection::ReadWrite));
            let protection = PROBES.with(|probes| probes.get());

            assert_eq!(anonymous_protection(&vmas, PAGE * 2), None);
            vmas.last_mut().unwrap().reservation = Reservation(1);
            assert!(reserved_and_absent(&table, &vmas, PAGE));
            let present = ProcessPageTable(Some(VirtAddr::new(PAGE)));
            assert!(!reserved_and_absent(&present, &vmas, PAGE));
            [reservation, protection]
        }

        #[test]
        fn a_private_first_touch_does_not_scan_unrelated_mappings() {
            let small = lookup_probes(64);
            let large = lookup_probes(4096);
            assert!(
                large.iter().zip(small).all(|(&large, small)| large <= small + 32),
                "reservation and protection probes with 64 VMAs: {small:?}; with 4096: {large:?}",
            );
        }
    }
}
"""


class AnonymousFaultVmas(unittest.TestCase):
    def test_first_touch_lookup_cost(self):
        source = (ROOT / "kernel/src/memory/anon_map.rs").read_text()
        start = source.index("pub(crate) fn reserved_and_absent(")
        end = source.index("pub(crate) use crate::ipc::shmem::resolve_page")
        with tempfile.TemporaryDirectory(prefix="breenix-anon-fault-") as directory:
            fixture = Path(directory) / "anon_fault.rs"
            binary = fixture.with_suffix("")
            fixture.write_text(FIXTURE.replace("LOOKUP_SOURCE", source[start:end]))
            subprocess.run(
                ["rustc", "--edition=2021", "-Dwarnings", "--test", str(fixture), "-o", str(binary)],
                check=True,
                timeout=30,
            )
            subprocess.run([binary], check=True, timeout=10)


if __name__ == "__main__":
    unittest.main()
