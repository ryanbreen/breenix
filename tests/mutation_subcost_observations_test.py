"""E24 531(w): emit bounded VFS sub-cost deltas through failure observations."""
from pathlib import Path
import unittest
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class MutationSubcostObservations(unittest.TestCase):
    def test_failure_only_records_include_inflight_and_aggregate_repeated_calls(self):
        module = ROOT / 'libs/libbreenix/src/suite_mutation_costs.rs'
        self.assertTrue(module.exists(), 'missing VFS sub-cost failure observation transport')
        source = r'''
#[path="MODULE"] mod costs;
fn row(started:u64, completed:u64, ns:u64)->String {
    format!("/ breenfs durable_generation=7\nmutation_cost mount=1 root=1 op=unlink phase=inode-edit started={started} completed={completed} total_ns={ns} last_start_ns=90 last_end_ns=100\n")
}
fn main() {
    let _reader = costs::Snapshot::read;
    let before=costs::Snapshot::parse(&row(10,10,100));
    // Successful cases must not even read the final snapshot.
    assert!(before.records("ui-testing","tree/enabled-flag",false,||panic!("success read")).is_empty());
    let after=costs::Snapshot::parse(&row(1011,1010,50100));
    let lines=before.records("ui-testing","tree/enabled-flag",true,||Some(after));
    assert_eq!(lines.len(),1,"{lines:?}");
    assert!(lines[0].starts_with("CASE_OBSERVATION ui-testing tree/enabled-flag op=vfs-unlink-inode-edit "),"{lines:?}");
    assert!(lines[0].contains("started=1001 completed=1000 total_ns=50000"),"{lines:?}");
    assert!(lines[0].contains("active=1"),"{lines:?}");
    assert!(lines[0].contains("scope=mount"),"{lines:?}");
    assert!(before.records("ui-testing","tree/enabled-flag",true,||Some(costs::Snapshot::parse(&row(10,10,100)))).is_empty());
    let missing=before.records("gui-apps","guskit/session-dialog",true,||None);
    assert_eq!(missing.len(),1); assert!(missing[0].contains("unavailable"));
    // A different mount is reported without subtracting another mount's history.
    let changed=costs::Snapshot::parse(&row(2,1,55).replace("mount=1 ","mount=2 "));
    let lines=before.records("libc","dirent-glob/readdir-many",true,||Some(changed));
    assert!(lines[0].contains("started=2 completed=1 total_ns=55"),"{lines:?}");
    // Only the root's rows participate; unrelated mount histories are excluded.
    let foreign=costs::Snapshot::parse(&row(50,50,100).replace("/ breenfs", "/mnt/other breenfs"));
    assert!(costs::Snapshot::parse("").records("ui-testing","tree/enabled-flag",true,||Some(foreign)).is_empty());
    // Arbitrary/malformed input cannot manufacture unbounded serial output.
    let many=(0..10000).map(|_|row(1011,1010,50100)).collect::<String>();
    let lines=before.records("ui-testing","tree/enabled-flag",true,||Some(costs::Snapshot::parse(&many)));
    assert!(lines.len()<=64 && lines.iter().all(|line|line.len()<384));
}
'''.replace('MODULE', str(module))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)
        scope = (ROOT / 'libs/libbreenix/src/suite_control.rs').read_text()
        self.assertIn('mutations.records(suite, case, failed,', scope)
        self.assertIn('mutations: mutation_costs::Snapshot', scope)
        self.assertIn('mutation_costs::Snapshot::read()', scope)


if __name__ == '__main__':
    unittest.main()
