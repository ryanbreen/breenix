"""Run the kernel's address selection on the host with counted VMA reads."""

from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]

FIXTURE = r"""
extern crate alloc;

mod memory {
    pub mod vma {
        pub const MMAP_REGION_START: u64 = 0x7000_0000_0000;
        pub const MMAP_REGION_END: u64 = MMAP_REGION_START + 0x4000_0000;

        pub struct Addr(pub u64);
        impl Addr {
            pub fn as_u64(&self) -> u64 {
                crate::READS.with(|reads| reads.set(reads.get() + 1));
                self.0
            }
        }

        pub struct Vma { pub start: Addr, pub end: Addr }
    }
}

mod process {
    use crate::memory::vma::Vma;

    #[derive(Default)]
    pub struct Process {
        pub vmas: Vec<Vma>,
        MMAP_FIELDS
    }
}

mod syscall {
    pub mod memory_common {
        PLACEMENT_SOURCE
    }
}

use memory::vma::{Addr, Vma, MMAP_REGION_END as TOP, MMAP_REGION_START as FLOOR};
use process::Process;
use syscall::memory_common::{place_below_hint, placement_pending, release_placement, PAGE_SIZE as P};

thread_local! { static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn process_with(ranges: impl IntoIterator<Item = (u64, u64)>) -> Process {
    Process {
        mmap_hint: TOP,
        vmas: ranges.into_iter().map(|(start, end)| Vma { start: Addr(start), end: Addr(end) }).collect(),
        ..Process::default()
    }
}

fn reads_after_reserving(count: u64, stride: u64) -> usize {
    // Highest addresses are occupied. In the fragmented fixture the holes
    // among the VMAs are also too small for the request.
    let mut process = process_with((0..count).rev().map(|index| {
        let end = TOP - index * stride * P;
        (end - P, end)
    }));
    let top = TOP - ((count - 1) * stride + 1) * P;
    assert_eq!(place_below_hint(&mut process, 2 * P), Some(top - 2 * P));
    READS.with(|reads| reads.set(0));
    assert_eq!(place_below_hint(&mut process, 2 * P), Some(top - 4 * P));
    READS.with(|reads| reads.get())
}

#[test]
fn placement_does_not_rescan_live_vmas() {
    let observed = [
        (reads_after_reserving(64, 1), reads_after_reserving(4096, 1)),
        (reads_after_reserving(64, 2), reads_after_reserving(4096, 2)),
    ];
    assert!(observed.iter().all(|&(small, large)| large <= small + 32),
        "VMA endpoint reads for 64 and 4096 live VMAs (dense, fragmented): {observed:?}");
}

#[test]
fn highest_fit_respects_pending_ranges_alignment_and_floor() {
    let mut process = process_with([(TOP - 4 * P, TOP - 2 * P), (FLOOR + 2 * P, TOP - 6 * P)]);
    process.mmap_hint = FLOOR;
    assert_eq!(place_below_hint(&mut process, P + 1), Some(TOP - 2 * P));
    assert_eq!(process.mmap_hint, FLOOR);
    assert_eq!(place_below_hint(&mut process, 2 * P), Some(TOP - 6 * P));
    assert!(placement_pending(&process, TOP - 6 * P, TOP - 5 * P));
    assert_eq!(place_below_hint(&mut process, 2 * P), Some(FLOOR));
    assert_eq!(place_below_hint(&mut process, P), None);
    release_placement(&mut process, TOP - 2 * P, TOP - P);
    assert!(!placement_pending(&process, TOP - 2 * P, TOP));
    assert_eq!(place_below_hint(&mut process, 2 * P), Some(TOP - 2 * P));
}
"""


class MmapPlacement(unittest.TestCase):
    def test_kernel_placement(self):
        common = (ROOT / "kernel/src/syscall/memory_common.rs").read_text()
        page_size = re.search(r"^pub const PAGE_SIZE: .*;$", common, re.MULTILINE).group()
        alignment = common[common.index("/// Round down to page size"):common.index("/// Check if address")]
        placement = common[common.index("/// The highest page-aligned range"):common.index("/// Pages mapped per")]
        process = (ROOT / "kernel/src/process/process.rs").read_text()
        fields = re.findall(r"^    pub(?:\(crate\))? mmap_\w+: [^\n]+,$", process, re.MULTILINE)
        source = FIXTURE.replace("MMAP_FIELDS", "\n".join(fields)).replace(
            "PLACEMENT_SOURCE", page_size + "\n" + alignment + placement
        )
        with tempfile.TemporaryDirectory(prefix="breenix-mmap-placement-") as tmp:
            fixture = Path(tmp) / "placement.rs"
            binary = fixture.with_suffix("")
            fixture.write_text(source)
            subprocess.run(
                ["rustc", "--edition=2021", "-Dwarnings", "--test", str(fixture), "-o", str(binary)],
                check=True, timeout=30,
            )
            subprocess.run([binary], check=True, timeout=10)


if __name__ == "__main__":
    unittest.main()
