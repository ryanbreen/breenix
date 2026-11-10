use libsmb::auth::spnego::decode_response;

#[test]
fn response_and_invalid_der_length() {
    let token = [
        0xa1, 0x0e, 0x30, 0x0c, 0xa0, 0x03, 0x0a, 0x01, 0x01, 0xa2, 0x05, 0x04, 0x03, 1, 2, 3,
    ];
    let response = decode_response(&token).unwrap();
    assert_eq!(response.state, Some(1));
    assert_eq!(response.token.as_deref(), Some(&[1, 2, 3][..]));
    let mut bad = token;
    bad[1] = 0x0f;
    assert!(decode_response(&bad).is_err());
}
