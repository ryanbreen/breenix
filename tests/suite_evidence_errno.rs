extern crate self as libbreenix;

#[path = "../libs/libbreenix/src/errno.rs"]
pub mod errno;
pub use errno::Errno;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;
pub mod crypto {
    pub use crate::sha256;
}
#[path = "../libs/libbreenix/src/suite_evidence.rs"]
pub mod evidence;

#[test]
fn syscall_failures_keep_their_errno_in_upload_records() {
    for (ret, suffix) in [
        (-110, "etimedout"),
        (-111, "econnrefused"),
        (-113, "ehostunreach"),
        (-32, "epipe"),
        (-4095, "errno-4095"),
    ] {
        for stage in ["socket", "connect", "send", "response-read", "response-poll", "lease-wait"] {
            let token = evidence::syscall_result(stage, ret).unwrap_err();
            assert_eq!(token, format!("{stage}-{suffix}"));
            assert!(token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        }
        let route = evidence::Route::parse("http://fixture.invalid/upload", "window-manager", false).unwrap();
        assert_eq!((&*route.host, route.port, &*route.authority), ("fixture.invalid", 80, "fixture.invalid"));
        let mut upload = evidence::Upload::new("window-manager", Ok(Some(route)));
        assert!(upload.screen_digests.is_empty());
        let token = evidence::syscall_result("connect", ret).unwrap_err();
        let line = upload.capture("startup", || Ok(evidence::Observation {
            width: 1, height: 1, record: [0; 1024], pixels: vec![1, 2, 3, 255],
        }), |_, _, _, _| Err(token.clone()));
        assert_eq!(line, format!("SUITE_EVIDENCE window-manager startup refused=connect-{suffix}"));
        assert_eq!(upload.capture("suite-final-panel", || panic!("startup refused"), |_, _, _, _| panic!()),
            format!("SUITE_EVIDENCE window-manager suite-final-panel refused=startup-connect-{suffix}"));
    }
    assert_eq!(evidence::syscall_result("send", 17).unwrap(), 17);
    assert_eq!(evidence::syscall_result("response-read", 0).unwrap(), 0);
}

#[test]
fn expired_lease_wait_refuses_before_transport() {
    assert_eq!(evidence::lease_ready(false), Err("lease-wait-expired".into()));
    assert_eq!(evidence::lease_ready(true), Ok(()));
}
