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

#[test]
fn ntlm_mechanism_list_mic_binds_the_der_list_and_direction() {
    use libsmb::auth::spnego::{encode_authenticated_response, verify_server_mic};
    // RFC 4178 section 5: DER MechTypeList = 30 0c 06 0a <NTLM OID>.
    // MS-NLMP 3.4.5.2 / 3.4.4.2, extended security, no KEY_EXCH,
    // sequence 0, exported key 00..0f. Independently evaluated with
    // Python hashlib.md5 and hmac.new(..., 'md5').
    let key = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    let expected = [
        0xa1, 0x1e, 0x30, 0x1c, 0xa2, 0x06, 0x04, 0x04, b'a', b'u', b't', b'h', 0xa3, 0x12, 0x04,
        0x10, 1, 0, 0, 0, 0x49, 0xea, 0x44, 0x2b, 0xeb, 0x10, 0x41, 0x47, 0, 0, 0, 0,
    ];
    let wire = encode_authenticated_response(b"auth", &key);
    assert_eq!(wire, expected);
    let client_mic = decode_response(&wire).unwrap().mic.unwrap();
    assert!(!verify_server_mic(&key, &client_mic));
    let mut server_mic = [
        1, 0, 0, 0, 0x0a, 0xfb, 0x34, 0x90, 0x3a, 0x3e, 0x4e, 0x54, 0, 0, 0, 0,
    ];
    assert!(verify_server_mic(&key, &server_mic));
    server_mic[4] ^= 1;
    assert!(!verify_server_mic(&key, &server_mic));
}
