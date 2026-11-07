"""Exercise startup publication through the production shared mapping."""
from pathlib import Path
import os
import subprocess
import unittest
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class StartupTrace(unittest.TestCase):
    def test_shared_first_stamp_survives_later_marks(self):
        rel = 'libs/libuitest/src/startup.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        if old:
            source = subprocess.run(['git','show',f'{old}:{rel}'],cwd=ROOT,capture_output=True,text=True)
            self.assertEqual(source.returncode, 0, source.stderr)
            source = source.stdout
        else:
            source = (ROOT/rel).read_text()
        run_rust(self, source + r'''
mod libbreenix { pub mod time {
 pub fn now_monotonic()->Result<std::time::Duration,()> {Ok(std::time::Duration::from_nanos(99))}
}}
#[test]fn independent_mappings_publish_first_sample(){
 let root=std::env::temp_dir().join(format!("startup-trace-{}",std::process::id()));
 std::fs::create_dir(&root).unwrap();
 let writer=Trace::create(&root).unwrap();let reader=Trace::open(&root).unwrap();
 assert_eq!(reader.snapshot().len(),PHASES.len());
 assert!(reader.snapshot().iter().all(|(_,ns)|*ns==0));
 writer.at("configure-begin",41);reader.at("configure-begin",42);
 assert_eq!(reader.snapshot()[0],("configure-begin",41));
 reader.mark("app-commit");assert_eq!(writer.snapshot()[18],("app-commit",99));
 reader.at("unknown",500);assert_eq!(writer.snapshot()[19].1,0);
 std::env::set_var("BREENIX_CONFIG_ROOT",&root);mark("scene-ready");
 assert_eq!(writer.snapshot()[13].1,99);
 drop(reader);drop(writer);std::fs::remove_dir_all(root).unwrap();
}
''')
if __name__ == '__main__': unittest.main()
