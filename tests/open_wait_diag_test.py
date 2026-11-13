"""Exercise the diagnostic branch's bounded per-open accounting on a host."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix="open-wait-diag-") as temp:
        source = Path(temp) / "check.rs"
        source.write_text('''
#[path = "''' + str(ROOT / "kernel/src/fs/open_wait/counters.rs") + '''"]
pub mod counters;
use counters::{Counters, Metric};
#[test]
fn open_accounting_is_owned_reset_and_additive() {
    let counters = Counters::new();
    assert!(counters.begin(7));
    assert!(!counters.begin(8), "a colliding open must not overwrite its owner");
    counters.add(8, Metric::Mmio, 999);
    counters.add(7, Metric::Namei, 40);
    counters.add(7, Metric::ReadGuard, 12);
    counters.add(7, Metric::Mmio, 10);
    counters.add(7, Metric::Mmio, 20);
    counters.add(7, Metric::Expired, 1);
    counters.add(7, Metric::ExpiredDone, 1);
    let snapshot = counters.finish(7).unwrap();
    assert_eq!(snapshot[Metric::Namei as usize], 40);
    assert_eq!(snapshot[Metric::ReadGuard as usize], 12);
    assert_eq!(snapshot[Metric::Mmio as usize], 30);
    assert_eq!(snapshot[Metric::Expired as usize], 1);
    assert_eq!(snapshot[Metric::ExpiredDone as usize], 1);
    assert!(counters.begin(8));
    assert_eq!(counters.finish(8).unwrap(), [0; counters::METRICS]);
}
''')
        binary = Path(temp) / "check"
        subprocess.run(["rustc", "--edition=2021", "--test", "-Dwarnings",
                        str(source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    main()
