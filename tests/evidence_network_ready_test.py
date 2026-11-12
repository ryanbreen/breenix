"""Run the uploader's production readiness loop with deterministic PNP snapshots."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class EvidenceNetworkReady(unittest.TestCase):
    def test_static_and_dhcp_readiness_before_transport(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        start = source.index('    let mut ready = false;', source.index('fn evidence_transfer('))
        end = source.index('    let ip = ', start)
        body = source[start:end].replace('std::fs::read_to_string', 'pnp')
        rust = r'''
use std::cell::RefCell;
thread_local! { static INPUT: RefCell<(Vec<Option<String>>, usize, usize)> = RefCell::new((vec![],0,0)); }
fn pnp(path: &str) -> Result<String, ()> {
    assert_eq!(path, "/proc/net/pnp");
    INPUT.with(|s| { let mut s=s.borrow_mut(); let i=s.1.min(s.0.len()-1); s.1+=1; s.0[i].clone().ok_or(()) })
}
mod libbreenix { pub mod types { pub struct Timespec { pub tv_sec:i64, pub tv_nsec:i64 } } }
mod nr { pub const NANOSLEEP:u64=35; }
mod raw {
    pub unsafe fn syscall2(n:u64, ptr:u64, zero:u64)->i64 {
        assert_eq!((n,zero),(super::nr::NANOSLEEP,0));
        let t=&*(ptr as *const super::libbreenix::types::Timespec);
        assert_eq!((t.tv_sec,t.tv_nsec),(0,100_000_000));
        super::INPUT.with(|s|s.borrow_mut().2+=1); 0
    }
}
mod evidence {
    pub fn syscall_result(_: &str, ret:i64)->Result<u64,String> { assert_eq!(ret,0); Ok(0) }
    LEASE
}
fn ready()->Result<(),String> { BODY Ok(()) }
fn run(rows: &[Option<&str>]) -> (Result<(),String>,usize) {
    INPUT.with(|s| *s.borrow_mut()=(rows.iter().map(|s|s.map(str::to_owned)).collect(),0,0));
    let result=ready(); let waits=INPUT.with(|s|s.borrow().2); (result,waits)
}
fn main() {
    assert_eq!(run(&[Some("#MANUAL\n")]), (Ok(()),0), "static slirp must reach socket creation");
    assert_eq!(run(&[Some("#PROTO: DHCP\nbootserver 192.0.2.1\n")]), (Ok(()),0));
    assert_eq!(run(&[Some("#PROTO: DHCP\n"),Some("#PROTO: DHCP\nbootserver 192.0.2.1\n")]), (Ok(()),1));
    for row in [None,Some("#PROTO: DHCP\n"),Some("garbage\n"),Some("#MANUAL-invalid\n")] {
        assert_eq!(run(&[row]), (Err("lease-wait-expired".into()),200));
    }
}
'''
        evidence = (ROOT / 'libs/libbreenix/src/suite_evidence.rs').read_text()
        lease = evidence[evidence.index('pub(super) fn lease_ready('):evidence.index('#[derive(Clone)]')]
        rust = rust.replace('LEASE', lease).replace('BODY', body)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(rust)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
