"""Run production config snapshots and the extracted GUI case boundary on the host."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class ConfigIsolation(unittest.TestCase):
    def compile_run(self, source, library):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            code = tmp / 'check.rs'
            code.write_text(source)
            for command in [
                ['rustc', '--edition=2021', '--crate-type=rlib', '--crate-name=libpixcheck', str(ROOT / 'libs/libpixcheck/src/lib.rs'), '-o', str(tmp / 'libpixcheck.rlib')],
                ['rustc', '--edition=2021', '--test', '-Dwarnings', str(code), '--extern', f'libpixcheck={tmp / "libpixcheck.rlib"}', '-o', str(tmp / 'check')],
                [str(tmp / 'check')],
            ]:
                if sys.platform == 'darwin' and command[0] == 'rustc': command = ['taskpolicy', '-b', *command]
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_path_map_and_recursive_hash_restore(self):
        source = (ROOT / 'tests/ui_config_isolation.rs').read_text().replace('../userspace/', str(ROOT / 'userspace') + '/').replace('../libs/', str(ROOT / 'libs') + '/')
        self.compile_run(source, True)

    def test_real_runner_boundary_restores_after_success_crash_timeout_and_early_error(self):
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        rel = 'libs/libbreenix/src/suite.rs'
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        boundary = item(source, 'fn run_case(')
        harness = r'''
use std::{fs, path::PathBuf, sync::Mutex};
static ROOT:Mutex<Option<PathBuf>>=Mutex::new(None);
pub struct Case{pub supervisor:Option<()>,pub fail:bool}
pub struct Feed;
pub struct Screen{pub refused:bool}
impl Screen{
    pub fn release_for_session(&mut self)->Result<(),String>{if self.refused{Err("display refused".into())}else{Ok(())}}
    pub fn reclaim_after_session(&mut self)->Result<(),String>{Ok(())}
}
#[derive(Debug)]pub enum Outcome{Pass{ms:u64},Fail{ms:u64,msg:String},Skip{msg:String}}
NOTES
pub fn run_case_body(case:&Case,_:u64,_:&str,_:&str,_:bool,_:&mut Screen,_:&mut Feed)->Outcome{
    fs::write(ROOT.lock().unwrap().as_ref().unwrap().join("etc/fonts.conf"), b"planted").unwrap();
    if case.fail{
        let root=ROOT.lock().unwrap();let root=root.as_ref().unwrap();
        for name in ["etc/hotkeys.conf","etc/bwm.conf"]{fs::write(root.join(name),b"planted").unwrap();}
        for dir in ["usr/share/applications","var/log"]{fs::create_dir_all(root.join(dir)).unwrap();}
        Outcome::Fail{ms:3,msg:"x".repeat(160)}
    }else{Outcome::Pass{ms:3}}
}
CONFIG
BOUNDARY
#[test]fn runner_restores_before_returning_a_verdict(){
    let root=std::env::temp_dir().join(format!("runner-config-{}",std::process::id()));
    fs::create_dir_all(root.join("etc")).unwrap();*ROOT.lock().unwrap()=Some(root.clone());
    for fail in [false,true]{
        fs::write(root.join("etc/fonts.conf"),b"default").unwrap();
        let outcome=run_case(&Case{supervisor:None,fail},3000,"test","planted",true,&mut Screen{refused:false},&mut Feed);
        assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(),b"default","global mutation survived GUI case");
        let Outcome::Fail{msg,..}=outcome else{panic!("global mutation passed")};
        for name in if fail{vec!["/etc/fonts.conf","/etc/hotkeys.conf","/etc/bwm.conf","/usr/share/applications","/var/log"]}else{vec!["/etc/fonts.conf"]}{assert!(msg.contains(name),"{msg}");}
    }
    // Non-GUI cases retain their existing behavior.
    run_case(&Case{supervisor:None,fail:false},3000,"test","non-gui",false,&mut Screen{refused:false},&mut Feed);
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(),b"planted");
    fs::write(root.join("etc/fonts.conf"),b"default").unwrap();
    assert!(matches!(run_case(&Case{supervisor:Some(()),fail:false},3000,"test","early",true,&mut Screen{refused:true},&mut Feed),Outcome::Fail{..}));
    assert_eq!(fs::read(root.join("etc/fonts.conf")).unwrap(),b"default");
    fs::remove_dir_all(root).unwrap();
}
'''
        config = ''
        if (ROOT / 'libs/libbreenix/src/suite_config.rs').exists():
            config = r'''
#[path="SOURCE"]pub mod production;
pub mod config{
    pub struct Snapshot(super::production::Snapshot);
    impl Snapshot{
        pub fn capture()->Result<Self,String>{super::production::Snapshot::capture_at(super::ROOT.lock().unwrap().as_ref().unwrap()).map(Self)}
        pub fn verify_restore(self)->Result<(),String>{self.0.verify_restore()}
    }
}
'''.replace('SOURCE', str(ROOT / 'libs/libbreenix/src/suite_config.rs'))
        notes = 'const MSG_MAX:usize=160;\n' + item(source, 'fn plain(') + '\n' + item(source, 'fn with_note(')
        self.compile_run(harness.replace('CONFIG', config).replace('BOUNDARY', boundary).replace('NOTES', notes), False)

if __name__ == '__main__': unittest.main()
