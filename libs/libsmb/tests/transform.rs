use libsmb::transform::{Cipher, Transform};

#[test]
fn header_and_authenticated_round_trips() {
    let key = [0x42; 16];
    let session = 0x0807060504030201;
    for cipher in [Cipher::Aes128Gcm, Cipher::Aes128Ccm] {
        let mut transform = Transform::new(cipher, key, session, [0x11, 0x22, 0x33]);
        let frame = transform.encrypt(b"hello SMB").unwrap();
        assert_eq!(&frame[..4], b"\xfdSMB");
        assert_eq!(&frame[20..36], &[0x11, 0x22, 0x33, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&frame[36..52], &[9, 0, 0, 0, 0, 0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(transform.decrypt(&frame).unwrap(), b"hello SMB");
        let next = transform.encrypt(b"hello SMB").unwrap();
        assert_ne!(&frame[20..36], &next[20..36]);
        for index in [4, 20, 52] {
            let mut bad = frame.clone();
            bad[index] ^= 1;
            assert!(transform.decrypt(&bad).is_err());
        }
        let other = Transform::new(cipher, key, session + 1, [0x11, 0x22, 0x33]);
        assert!(other.decrypt(&frame).is_err());
        assert!(transform.decrypt(&frame[..51]).is_err());
        let mut bad_size = frame.clone();
        bad_size[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(transform.decrypt(&bad_size).is_err());
    }
}
