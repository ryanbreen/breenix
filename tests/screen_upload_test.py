"""Run the guest checkpoint queue and ordered sender against host file/HTTP sinks."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]

class ScreenUpload(unittest.TestCase):
    def test_known_digests_order_parts_and_budget(self):
        code = r'''
#![allow(dead_code)]
extern crate self as libbreenix;
pub mod crypto { #[path="SHA"] pub mod sha256; }
#[path="EVIDENCE"] mod evidence;
#[path="QUEUE"] mod queue;
#[test] fn checkpoint_order_known_suppression_and_shared_sender() {
    let root = std::path::PathBuf::from(std::env::var("QUEUE_DIR").unwrap());
    let path = root.join("pending");
    let pixels = [1,2,3,255];
    let digest = libpixcheck::sha256(&pixels);
    let hex: String = digest.iter().map(|b|format!("{b:02x}")).collect();
    let record = screen_capture::Record {suite:"gpu",case:"draw/one",checkpoint:"end",width:1,height:1,digest,pixels:&pixels};
    assert!(!queue::queue(&path, &hex, &record).unwrap());
    assert!(!path.exists());
    assert!(queue::queue(&path, "", &record).unwrap());
    let noise: Vec<u8> = (0..600_000u32).flat_map(|n|[n as u8,(n>>8)as u8,(n>>16)as u8,255]).collect();
    let second = screen_capture::Record {suite:"gpu",case:"draw/two",checkpoint:"intent",width:1000,height:600,
        digest:libpixcheck::sha256(&noise),pixels:&noise};
    queue::queue(&path, "", &second).unwrap();
    let route = evidence::Route::parse("http://fixture.invalid/upload/screens", "gpu", false).unwrap();
    let mut upload = evidence::Upload::new("gpu", Ok(Some(route)));
    let mut sent = 0usize;
    let panel = || Ok(evidence::Observation {width:1,height:1,record:[0;1024],pixels:vec![1,2,3,255]});
    assert!(upload.capture("startup", panel, |_,_,body,_| {sent+=body.len(); Ok(())}).contains(" sha256="));
    let mut requests = Vec::new();
    let lines = queue::flush(&path, &mut upload, |_,path,body,digest| {
        assert!(body.len() <= evidence::MAX_UPLOAD);
        assert_eq!(digest, crypto::sha256::sha256(body));
        sent += body.len();
        requests.push((path.to_string(),body.to_vec())); Ok(())
    });
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("SCREEN_UPLOAD gpu draw/one end first=0 parts=1 bytes=512 sha256="));
    assert!(lines[1].starts_with("SCREEN_UPLOAD gpu draw/two intent first=1 parts=2 bytes="));
    assert_eq!(requests.iter().map(|v|v.0.as_str()).collect::<Vec<_>>(),
        ["/upload/screens/pictures/0","/upload/screens/pictures/1","/upload/screens/pictures/2"]);
    let joined: Vec<u8> = requests[1..].iter().flat_map(|v|v.1.iter().copied()).collect();
    let decoded = screen_capture::decode(&joined).unwrap().0;
    assert_eq!(decoded.pixels,noise);
    assert!(!path.exists());
    // A later checkpoint may name pixels already retained by this boot.
    let alias = screen_capture::Record {suite:"gpu",case:"draw/alias",checkpoint:"intent",..record};
    queue::queue(&path, "", &alias).unwrap();
    let before = sent;
    let aliases = queue::flush(&path, &mut upload, |_,_,body,_| {sent+=body.len(); Ok(())});
    assert!(aliases.is_empty(), "duplicate picture was sent again");
    assert_eq!(sent, before);
    assert!(!path.exists());
    // The budget includes startup/final panel bodies and checkpoint records.
    assert!(upload.capture("suite-final-panel", panel, |_,_,body,_| {sent+=body.len(); Ok(())}).contains(" sha256="));
    let raw = vec![7;2<<20];
    let mut refused = false;
    for _ in 0..40 {
        let line = queue::picture(&mut upload, "gpu draw/three end", &raw, |_,_,body,_| {sent+=body.len(); Ok(())});
        if line.0.contains("refused=") { assert!(line.0.ends_with("refused=total-bound")); refused=true; break; }
    }
    assert!(refused);
    let over = vec![0; evidence::MAX_TOTAL - sent + 1];
    assert!(queue::picture(&mut upload, "gpu draw/three end", &over,
        |_,_,_,_|panic!("panel and checkpoint bytes must share the budget")).0.ends_with("refused=total-bound"));
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            for key, value in [('SHA','libs/libbreenix/src/crypto/sha256.rs'),
                               ('EVIDENCE','libs/libbreenix/src/suite_evidence.rs'),
                               ('QUEUE','libs/libbreenix/src/suite_screen_upload.rs')]:
                code = code.replace('"'+key+'"', '"'+str(ROOT/value)+'"')
            (path/'test.rs').write_text(code)
            priority = ['taskpolicy','-b'] if sys.platform == 'darwin' else []
            commands = [
                ['rustc','--edition=2021','--crate-name','libpixcheck','--crate-type=rlib',str(ROOT/'libs/libpixcheck/src/lib.rs'),'-o',str(path/'liblibpixcheck.rlib')],
                ['rustc','--edition=2021','--crate-name','screen_capture','--crate-type=rlib',str(ROOT/'libs/screen-capture/src/lib.rs'),'--extern','libpixcheck='+str(path/'liblibpixcheck.rlib'),'-L',str(path),'-o',str(path/'libscreen_capture.rlib')],
                ['rustc','--edition=2021','--test','-Dwarnings',str(path/'test.rs'),'--extern','libpixcheck='+str(path/'liblibpixcheck.rlib'),'--extern','screen_capture='+str(path/'libscreen_capture.rlib'),'-L',str(path),'-o',str(path/'test')]]
            for cmd in commands:
                result = subprocess.run(priority+cmd,capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)
            result = subprocess.run([str(path/'test')],capture_output=True,text=True,env=dict(os.environ,QUEUE_DIR=tmp))
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)

if __name__ == '__main__': unittest.main()
