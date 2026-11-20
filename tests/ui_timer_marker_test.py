#!/usr/bin/env python3
"""Force a reader between the timer marker's open and write."""
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class TimerMarkerTest(unittest.TestCase):
    def test_only_complete_epochs_are_visible(self):
        app = (ROOT / 'userspace/programs/src/uitest_timer_app.rs').read_text()
        publication = re.search(
            r'if let Some\(root\) = std::env::var_os\("BREENIX_CONFIG_ROOT"\) \{\n'
            r'(.*?)\n +\}', app, re.S)
        self.assertIsNotNone(publication, 'timer marker publication not found')
        body = publication.group(1).replace('std::fs::', 'observed_fs::')
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            source = work / 'marker.rs'
            source.write_text(r'''
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static EPOCH: AtomicU64 = AtomicU64::new(2);

mod breengel {
    pub mod clock {
        pub fn epoch() -> u64 { super::super::EPOCH.load(super::super::Relaxed) }
    }
}

pub mod observed_fs {
    use std::{fs, io::{self, Write}, path::Path};

    pub fn write(path: impl AsRef<Path>, value: impl AsRef<[u8]>) -> io::Result<()> {
        let path = path.as_ref();
        let marker = path.with_file_name("timer-backpressure");
        let before = read_marker(&marker)?;
        let mut file = fs::File::create(path)?;
        assert_eq!(read_marker(&marker)?, before, "marker changed before the epoch was written");
        file.write_all(value.as_ref())
    }

    fn read_marker(marker: &Path) -> io::Result<Option<String>> {
        match fs::read_to_string(marker) {
            Ok(epoch) => Ok(Some(epoch)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub use fs::rename;
}

fn publish(root: &Path) -> Result<(), String> {
''' + body + r'''
    Ok(())
}

fn main() {
    let root = std::env::args_os().nth(1).unwrap();
    let root = Path::new(&root);
    let marker = root.join("timer-backpressure");
    for epoch in [2, 3] {
        EPOCH.store(epoch, Relaxed);
        publish(root).unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), epoch.to_string());
    }
}
''')
            binary = work / 'marker'
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', str(source),
                            '-o', str(binary)], check=True)
            subprocess.run([str(binary), str(work)], check=True)


if __name__ == '__main__':
    unittest.main()
