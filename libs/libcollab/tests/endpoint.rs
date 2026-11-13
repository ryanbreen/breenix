use libcollab::endpoint::{parse_endpoint, COLLAB_PORT};

#[test]
fn parses_ipv4_with_optional_port() {
    assert_eq!(parse_endpoint("192.168.1.42"), Ok(([192, 168, 1, 42], COLLAB_PORT)));
    assert_eq!(parse_endpoint("0.0.0.0:65535"), Ok(([0, 0, 0, 0], 65535)));
}

#[test]
fn rejects_invalid_endpoints() {
    for value in ["256.1.2.3", "1.2.3", "1.2.3.4:0", "1.2.3.4:65536", "1.2.3.4:12x", "1.2.3.4:", "1.2.3.4.5", "1.2.3.4 "] {
        assert!(parse_endpoint(value).is_err(), "{value}");
    }
}
