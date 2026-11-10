use libsmb::{decode_negotiate_response, encode_negotiate_request, frame, unframe, DialectCap, NegotiateContext};

const DEFAULT_REQUEST: &[u8] = &[
    0xfe,0x53,0x4d,0x42, 0x40,0,0,0, 0,0,0,0, 0,0,1,0,
    0,0,0,0, 0,0,0,0, 0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    36,0,2,0,1,0,0,0, 0x40,0,0,0,
    0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,
    104,0,0,0,2,0,0,0, 2,3,0x11,3,
    1,0,38,0,0,0,0,0, 1,0,32,0,1,0,
    0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,
    16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,
    0,0, 2,0,6,0,0,0,0,0, 2,0,2,0,1,0,
];
const CAPPED_REQUEST: &[u8] = &[
    0xfe,0x53,0x4d,0x42, 0x40,0,0,0, 0,0,0,0, 0,0,1,0,
    0,0,0,0, 0,0,0,0, 0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    36,0,1,0,1,0,0,0, 0x40,0,0,0,
    0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,
    0,0,0,0,0,0,0,0, 2,3,
];
const RESPONSE: &[u8] = &[
    0xfe,0x53,0x4d,0x42, 64,0,0,0, 0,0,0,0, 0,0,1,0,
    1,0,0,0, 0,0,0,0, 0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    65,0,3,0,0x11,3,2,0, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0x40,0,0,0, 0,0,1,0, 0,0,1,0, 0,0,1,0,
    0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0,
    128,0,0,0, 128,0,0,0,
    1,0,6,0,0,0,0,0, 1,0,0,0,1,0, 0,0,
    2,0,4,0,0,0,0,0, 1,0,2,0,
];
#[test]
fn requests_match_wire_bytes() {
    let guid = core::array::from_fn(|i| i as u8);
    let salt = core::array::from_fn(|i| i as u8);
    assert_eq!(encode_negotiate_request(DialectCap::Default, guid, salt), DEFAULT_REQUEST);
    assert_eq!(encode_negotiate_request(DialectCap::Smb302, guid, salt), CAPPED_REQUEST);
}
#[test]
fn response_and_truncation() {
    let response = decode_negotiate_response(RESPONSE).unwrap();
    assert_eq!(response.dialect, 0x0311);
    assert_eq!(response.security_mode, 3);
    assert_eq!(response.contexts, vec![NegotiateContext::Preauth { algorithms: vec![1], salt: vec![] }, NegotiateContext::Encryption { ciphers: vec![2] }]);
    for n in 0..RESPONSE.len() { assert!(decode_negotiate_response(&RESPONSE[..n]).is_err(), "prefix {n}"); }
}
#[test]
fn direct_tcp_limits_and_truncation() {
    let framed = frame(&[1,2,3]).unwrap();
    assert_eq!(framed, [0,0,0,3,1,2,3]);
    assert_eq!(unframe(&framed).unwrap(), &[1,2,3]);
    assert!(unframe(&framed[..6]).is_err());
    assert!(frame(&vec![0; 16 * 1024 * 1024]).is_err());
    assert!(unframe(&[1,0,0,0]).is_err());
}
