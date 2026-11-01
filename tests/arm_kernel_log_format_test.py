#!/usr/bin/env python3
"""Compile the ARM logger against a captured sink and inspect its audit records."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ArmKernelLogFormat(unittest.TestCase):
    def test_info_and_warning_records_retain_level_and_target(self):
        source = (ROOT / 'kernel/src/serial_aarch64.rs').read_text()
        start = source.index('impl log::Log for KernelLogger {')
        logger = source[start:source.index('\n}', start) + 2]
        harness = r'''
use log::Log;
static CAPTURE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
mod log {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Level { Warn, Info }
    impl std::fmt::Display for Level {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.pad(match self { Self::Warn => "WARN", Self::Info => "INFO" })
        }
    }
    pub struct Metadata(pub Level);
    impl Metadata { pub fn level(&self) -> Level { self.0 } }
    pub struct Record { pub meta: Metadata }
    impl Record {
        pub fn metadata(&self) -> &Metadata { &self.meta }
        pub fn level(&self) -> Level { self.meta.0 }
        pub fn target(&self) -> &str { "audit" }
        pub fn args(&self) -> &str { "kernel-restart-probe" }
    }
    pub trait Log {
        fn enabled(&self, meta: &Metadata) -> bool;
        fn log(&self, record: &Record);
        fn flush(&self);
    }
}
mod serial_line {
    pub struct TeeLine;
    impl TeeLine { pub fn with_level(_: crate::log::Level) -> Self { Self } }
    impl std::fmt::Write for TeeLine {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            crate::CAPTURE.lock().unwrap().push_str(s); Ok(())
        }
    }
}
struct KernelLogger;
LOGGER
fn main() {
    let logger = KernelLogger;
    logger.log(&log::Record { meta: log::Metadata(log::Level::Info) });
    assert_eq!(&*CAPTURE.lock().unwrap(), "[ INFO] audit: kernel-restart-probe\n",
               "ARM INFO record lost its level/target framing");
    CAPTURE.lock().unwrap().clear();
    logger.log(&log::Record { meta: log::Metadata(log::Level::Warn) });
    assert_eq!(&*CAPTURE.lock().unwrap(), "[ WARN] audit: kernel-restart-probe\n");
    logger.flush();
}
'''.replace('LOGGER', logger)
        with tempfile.TemporaryDirectory(prefix='arm-log-format-') as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
