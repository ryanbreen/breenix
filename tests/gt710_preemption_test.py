"""Execute the native owner locks with observable per-thread preemption state."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativePreemption(unittest.TestCase):
    def test_session_and_pramin_pin_the_holder(self):
        root = ROOT / 'kernel/src/gpu/gk208'
        declarations = []
        for file, name in [('mod.rs', 'PRAMIN_LOCK'), ('ce/process.rs', 'SESSION')]:
            line = next(line for line in (root / file).read_text().splitlines()
                        if line.startswith('static ' + name + ':'))
            declarations.append(line.replace('super::super::lock::', 'native_lock::')
                               .replace('lock::Mutex', 'native_lock::Mutex')
                               .replace('native_native_lock::', 'native_lock::')
                               .replace('Option<Session>', 'Option<()>'))
        locking = ('#[path = "' + str(root / 'lock.rs') + '"] mod native_lock;'
                   if (root / 'lock.rs').exists() else '')
        fixture = '#![cfg(test)]\n' + locking + '\n' + '\n'.join(declarations) + r"""
mod per_cpu {
    std::thread_local! { static COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    pub fn preempt_disable() { COUNT.with(|v| v.set(v.get()+1)); }
    pub fn preempt_enable() { COUNT.with(|v| v.set(v.get().checked_sub(1).unwrap())); }
    pub fn count()->u32 { COUNT.with(|v|v.get()) }
}
#[test] fn holder_cannot_be_preempted_and_early_return_restores_state() {
    fn early()->Result<(),()> {
        let _session=SESSION.lock();
        assert_eq!(per_cpu::count(),2,"SESSION must pin even an already pinned caller");
        { let _pramin=PRAMIN_LOCK.lock(); assert_eq!(per_cpu::count(),3,"PRAMIN must pin its holder"); }
        assert_eq!(per_cpu::count(),2);
        Err(())
    }
    per_cpu::preempt_disable();
    assert!(early().is_err()); assert_eq!(per_cpu::count(),1);
    per_cpu::preempt_enable(); assert_eq!(per_cpu::count(),0);
    { let _session=SESSION.lock(); assert_eq!(per_cpu::count(),1); }
    assert_eq!(per_cpu::count(),0);
}
"""
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'Cargo.toml').write_text('[package]\nname="native-lock-check"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nspin="0.9"\n[lib]\npath="fixture.rs"\n')
            (path / 'fixture.rs').write_text(fixture)
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings')
            subprocess.run(priority + ['cargo', 'test', '--quiet', '--manifest-path', str(path / 'Cargo.toml')],
                           env=env, check=True)

if __name__ == '__main__': unittest.main()
