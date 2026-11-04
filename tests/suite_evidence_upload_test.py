"""Execute the shared suite uploader with host capture and HTTP sinks."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SuiteEvidence(unittest.TestCase):
    def test_route_bounds_digest_and_optional_upload(self):
        source = r'''
extern crate self as libbreenix;
pub mod crypto { #[path = "SHA"] pub mod sha256; }
#[path = "EVIDENCE"] mod evidence;
use evidence::{Observation, Route, Upload, MAX_TOTAL, MAX_UPLOAD};
fn route(sequence: bool) -> Route {
    Route::parse("http://fixture.invalid:8080/upload/lease\n", "window-manager", sequence).unwrap()
}
fn picture(pixels: Vec<u8>, width: u32, height: u32) -> Observation {
    Observation { pixels, width, height, record: [0x5a; 1024] }
}
#[test] fn parses_optional_port_and_scopes_only_sequences() {
    let r = route(false);
    assert_eq!((&*r.host, r.port, &*r.authority, &*r.path),
        ("fixture.invalid", 8080, "fixture.invalid:8080", "/upload/lease"));
    assert_eq!(route(true).path, "/upload/lease/suites/window-manager");
    assert_eq!(Route::parse("http://127.0.0.1/a", "smoke", false).unwrap().port, 80);
    // No target-specific route prefix is required by this shared facility.
    for bad in ["https://fixture.invalid/a", "http://fixture.invalid", "http://u@fixture.invalid/a",
        "http://fixture.invalid:0/a", "http://fixture.invalid:65536/a", "http://fixture.invalid:bad/a",
        "http://fixture.invalid/a\r\nX-Foo: injected", "http://fixture.invalid/a?x=1",
        "http://fixture.invalid/a#part", "http://fixture.invalid/a b", "http:///a",
        "http://fixture.invalid/a\0"] {
        assert!(Route::parse(bad, "window-manager", false).is_err(), "{bad:?}");
    }
    assert!(Route::parse(&format!("http://fixture.invalid/{}", "x".repeat(2048)), "smoke", false).is_err());
}
#[test] fn absent_and_invalid_routes_record_without_capture_or_send() {
    for (configured, reason) in [(Ok(None), "skipped=no-route"),
        (Err("route-read".into()), "refused=route-read")] {
        let mut upload = Upload::new("window-manager", configured);
        for label in ["startup", "suite-final-panel"] {
            let line = upload.capture(label, || panic!("route must be checked before readback"),
                |_, _, _, _| panic!("route must be checked before sending"));
            assert_eq!(line, format!("SUITE_EVIDENCE window-manager {label} {reason}"));
        }
    }
}
#[test] fn retains_observation_and_exact_pixels_with_serial_and_http_digest() {
    let mut upload = Upload::new("window-manager", Ok(Some(route(true))));
    for (label, suffix) in [("startup", ""), ("suite-final-panel", "/final")] {
        let pixels = vec![0, 1, 2, 255, 3, 4, 5, 255];
        let line = upload.capture(label, || Ok(picture(pixels.clone(), 2, 1)),
            |r, path, body, digest| {
                assert_eq!(r.host, "fixture.invalid");
                assert_eq!(path, format!("/upload/lease/suites/window-manager{suffix}"));
                assert_eq!(digest, crypto::sha256::sha256(body));
                assert!(body.starts_with(b"SUITE_SCANOUT v=1 width=2 height=1 encoding=raw record=1024\n"));
                assert!(body.ends_with(&pixels));
                let header = body.iter().position(|&b| b == b'\n').unwrap() + 1;
                assert_eq!(&body[header..header+1024], &[0x5a;1024]);
                std::fs::write(std::env::var("EVIDENCE_BODY").unwrap(), body).unwrap();
                Ok(())
            });
        let body = std::fs::read(std::env::var("EVIDENCE_BODY").unwrap()).unwrap();
        let digest: String = crypto::sha256::sha256(&body).iter().map(|b|format!("{b:02x}")).collect();
        assert_eq!(line, format!("SUITE_EVIDENCE window-manager {label} bytes={} sha256={digest}",body.len()));
        println!("{line}");
    }
    // Existing native text RLE, chosen only when smaller than raw canonical pixels.
    let line = upload.capture("suite-final-panel", || Ok(picture([0,1,2,255].repeat(1024), 32, 32)),
        |_,_,body,_| { assert!(body.starts_with(b"SUITE_SCANOUT v=1 width=32 height=32 encoding=rle record=1024\n"));
            assert!(body.ends_with(b"400:ff020100\n")); Ok(()) });
    assert!(line.contains(" sha256="));
}
#[test] fn refuses_complete_over_bound_data_without_truncation_or_send() {
    let mut upload = Upload::new("window-manager", Ok(Some(route(false))));
    // G28 and G2: a complete image above the old slot size is valid.
    let pixels: Vec<u8> = (0..600_000u32).flat_map(|n| [n as u8,(n>>8)as u8,(n>>16)as u8,255]).collect();
    let line = upload.capture("startup", || Ok(picture(pixels.clone(), 1000, 600)),
        |_,_,body,_| { assert!(body.ends_with(&pixels)); Ok(()) });
    assert!(line.contains(" sha256="));
    // A picture's header and record also count against the fixed boot bound.
    let pixels: Vec<u8> = (0..(MAX_TOTAL / 4) as u32).flat_map(|n| [n as u8,(n>>8)as u8,(n>>16)as u8,255]).collect();
    let line = upload.capture("startup", || Ok(picture(pixels, 4096, 4096)),
        |_,_,_,_| panic!("over-bound data must not send even a prefix"));
    assert_eq!(line, "SUITE_EVIDENCE window-manager startup refused=upload-bound");
    let line = upload.capture("startup", || Ok(picture(vec![0;4], u32::MAX, u32::MAX)),
        |_,_,_,_| panic!("dimensions must refuse"));
    assert!(line.contains("refused=dimensions"));
    let line = upload.capture("startup", || Ok(picture(vec![0;3], 1, 1)),
        |_,_,_,_| panic!("short pixels must refuse"));
    assert!(line.contains("refused=pixels"));
    let line = upload.capture("startup", || Ok(picture(vec![0;4], 1, 1)),
        |_,_,_,_| panic!("noncanonical pixels must refuse"));
    assert!(line.contains("refused=pixels"));
    // Drive the cumulative bound with real bodies rather than overriding accounting.
    let mut upload = Upload::new("window-manager", Ok(Some(route(false))));
    let raw: Vec<u8> = (0..250_000u32).flat_map(|n| [n as u8,(n>>8)as u8,(n>>16)as u8,255]).collect();
    let mut sent = 0; let mut refused = false;
    for _ in 0..100 {
        let line = upload.capture("startup", || Ok(picture(raw.clone(), 500, 500)),
            |_,_,body,_| { assert!(body.len() <= MAX_UPLOAD); sent += body.len(); Ok(()) });
        if line.contains("refused=") { assert_eq!(line, "SUITE_EVIDENCE window-manager startup refused=total-bound"); refused=true; break; }
    }
    assert!(refused); assert!(sent <= MAX_TOTAL); assert!(sent + raw.len() > MAX_TOTAL);
}
#[test] fn producer_and_transport_errors_remain_records() {
    let mut upload = Upload::new("window-manager", Ok(Some(route(false))));
    assert_eq!(upload.capture("startup", || Err("readback".into()), |_,_,_,_|panic!()),
        "SUITE_EVIDENCE window-manager startup refused=readback");
    assert_eq!(upload.capture("suite-final-panel", || Ok(picture(vec![1,2,3,255],1,1)),
        |_,_,_,_|Err("http-status".into())), "SUITE_EVIDENCE window-manager suite-final-panel refused=http-status");
}
'''.replace('SHA', str(ROOT / 'libs/libbreenix/src/crypto/sha256.rs')).replace(
            '"EVIDENCE"', '"' + str(ROOT / 'libs/libbreenix/src/suite_evidence.rs') + '"')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            check = path / 'check.rs'
            check.write_text(source)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(check), '-o', str(path / 'check')]
            if sys.platform == 'darwin': command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True,
                                    env=dict(os.environ, CARGO_BUILD_JOBS='4'))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'check'), '--test-threads=1'], capture_output=True, text=True,
                                    env=dict(os.environ, EVIDENCE_BODY=str(path / 'body')))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__': unittest.main()
