use libbreenix::crypto::{hmac::hmac_sha256, sha512::sha512};
use libsmb::security::{derive_keys, sign, verify, Dialect, PreauthHash};

#[test]
fn key_derivation_uses_literal_dialect_inputs() {
    let key = [0x42; 16];
    let hash = [0x5a; 64];
    let expected = |input: &[u8]| -> [u8; 16] { hmac_sha256(&key, input)[..16].try_into().unwrap() };
    let keys = derive_keys(Dialect::Smb302, &key, &hash);
    assert_eq!(keys.signing, expected(b"\0\0\0\x01SMB2AESCMAC\0\0SmbSign\0\0\0\0\x80"));
    assert_eq!(keys.application, expected(b"\0\0\0\x01SMB2APP\0\0SmbRpc\0\0\0\0\x80"));
    assert_eq!(keys.encryption, expected(b"\0\0\0\x01SMB2AESCCM\0\0ServerIn \0\0\0\0\x80"));
    assert_eq!(keys.decryption, expected(b"\0\0\0\x01SMB2AESCCM\0\0ServerOut\0\0\0\0\x80"));
    let keys = derive_keys(Dialect::Smb311, &key, &hash);
    for (actual, prefix) in [
        (keys.signing, b"\0\0\0\x01SMBSigningKey\0\0".as_slice()),
        (keys.application, b"\0\0\0\x01SMBAppKey\0\0".as_slice()),
        (keys.encryption, b"\0\0\0\x01SMBC2SCipherKey\0\0".as_slice()),
        (keys.decryption, b"\0\0\0\x01SMBS2CCipherKey\0\0".as_slice()),
    ] {
        let mut input = prefix.to_vec();
        input.extend_from_slice(&hash);
        input.extend_from_slice(b"\0\0\0\x80");
        assert_eq!(actual, expected(&input));
    }
}

#[test]
fn preauthentication_chain_is_stepwise() {
    let mut chain = PreauthHash::new();
    let first = [vec![0; 64], b"negotiate request".to_vec()].concat();
    chain.update(b"negotiate request");
    assert_eq!(*chain.value(), sha512(&first));
    let second = [sha512(&first).to_vec(), b"session setup request".to_vec()].concat();
    chain.update(b"session setup request");
    assert_eq!(*chain.value(), sha512(&second));
}

#[test]
fn signing_detects_changes_and_wrong_keys() {
    let key = [7; 16];
    let mut message = vec![0; 80];
    message[..4].copy_from_slice(b"\xfeSMB");
    message[4..6].copy_from_slice(&64u16.to_le_bytes());
    message[64..].copy_from_slice(b"message contents");
    sign(&key, &mut message).unwrap();
    assert_ne!(message[16] & 8, 0);
    assert!(verify(&key, &message));
    assert!(!verify(&[8; 16], &message));
    message[70] ^= 1;
    assert!(!verify(&key, &message));
    assert!(!verify(&key, &message[..63]));
}
