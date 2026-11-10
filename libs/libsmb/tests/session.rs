//! Canned messages from MS-SMB2 2.2 and client processing in 3.2.4/3.2.5.
//! Fixtures are written from the wire layouts, not captured from a server.
use libbreenix::crypto::sha512::sha512;
use libsmb::{
    msg::{FileId, ReadRequest, ReadResponse, WriteRequest},
    security::{derive_keys, sign, verify, Dialect, SessionKeys},
    session::{Request, Session},
    transform::{Cipher, Transform},
    DialectCap, SyncHeader,
};

const HEADER: [u8; 64] = [
    0xfe, 0x53, 0x4d, 0x42, 64, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];
const NEGOTIATE: &[u8] = &[
    65, 0, 3, 0, 0x11, 3, 2, 0, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
    0x33, 0x33, 0x33, 0x33, 0x33, 0x44, 0, 0, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0,
    1, 0, 0, 0, 2, 0, 4, 0, 0, 0, 0, 0, 1, 0, 2, 0,
];
const TREE: &[u8] = &[16, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 1, 0x1f, 0];
const READ: &[u8] = &[
    17, 0, 80, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'a', b'b', b'c',
];
const VALIDATE: &[u8] = &[
    49, 0, 0, 0, 4, 2, 0x14, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 112, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x44,
    0, 0, 0, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
    0x33, 0x33, 3, 0, 2, 3,
];
const FILE: FileId = FileId {
    persistent: 1,
    volatile: 2,
};

fn response(command: u16, id: u64, tree: u32, credits: u16, body: &[u8]) -> Vec<u8> {
    let mut b = HEADER.to_vec();
    b[12..14].copy_from_slice(&command.to_le_bytes());
    b[14..16].copy_from_slice(&credits.to_le_bytes());
    b[24..32].copy_from_slice(&id.to_le_bytes());
    b[36..40].copy_from_slice(&tree.to_le_bytes());
    b.extend_from_slice(body);
    b
}
fn chain(hash: [u8; 64], b: &[u8]) -> [u8; 64] {
    sha512(&[hash.as_slice(), b].concat())
}
fn authenticated(cap: DialectCap, flags: u16) -> (Session, SessionKeys) {
    let mut s = Session::new(cap, [0x11; 16], [0x22; 32], [1, 2, 3]);
    let req = s.build(Request::Negotiate).unwrap();
    assert!(s.build(Request::Negotiate).is_err());
    let mut hash = chain([0; 64], &req);
    let mut neg = response(0, 0, 0, 6, NEGOTIATE);
    neg[40..48].fill(0);
    if cap == DialectCap::Smb302 {
        neg[68..72].copy_from_slice(&[2, 3, 0, 0]);
        neg.truncate(128);
        neg[124..128].fill(0);
    }
    s.consume(&neg).unwrap().unwrap();
    hash = chain(hash, &neg);
    let req = s
        .build(Request::SessionSetup {
            token: &[0x60, 0],
            session_key: None,
        })
        .unwrap();
    assert_eq!(SyncHeader::decode(&req).unwrap().message_id, 1);
    hash = chain(hash, &req);
    let mut challenge = response(1, 1, 0, 1, &[9, 0, 0, 0, 72, 0, 2, 0, 0xa1, 0]);
    challenge[8..12].copy_from_slice(&[0x16, 0, 0, 0xc0]);
    s.consume(&challenge).unwrap().unwrap();
    hash = chain(hash, &challenge);
    assert_eq!(s.session_id(), 9);
    let req = s
        .build(Request::SessionSetup {
            token: &[0xa1, 0],
            session_key: Some([0x42; 16]),
        })
        .unwrap();
    hash = chain(hash, &req);
    let dialect = if cap == DialectCap::Smb302 {
        Dialect::Smb302
    } else {
        Dialect::Smb311
    };
    let keys = derive_keys(dialect, &[0x42; 16], &hash);
    let mut done = response(1, 2, 0, 1, &[9, 0, 0, 0, 0, 0, 0, 0]);
    done[66..68].copy_from_slice(&flags.to_le_bytes());
    sign(&keys.signing, &mut done).unwrap();
    s.consume(&done).unwrap().unwrap();
    if cap != DialectCap::Smb302 {
        assert_eq!(s.preauth_hash(), &hash);
    }
    (s, keys)
}
fn connect(s: &mut Session, keys: &SessionKeys, encrypt: bool) {
    let req = s.build(Request::TreeConnect("\\\\server\\share")).unwrap();
    assert!(verify(&keys.signing, &req));
    let mut tree = response(3, 3, 7, 1, TREE);
    if encrypt {
        tree[69] = 0x80;
    }
    sign(&keys.signing, &mut tree).unwrap();
    s.consume(&tree).unwrap().unwrap();
    assert_eq!(s.tree_id(), Some(7));
}

#[test]
fn message_ids_spend_charge_and_pending_keeps_async_correlation() {
    let (mut s, keys) = authenticated(DialectCap::Default, 0);
    connect(&mut s, &keys, false);
    let req = s
        .build(Request::Read(ReadRequest {
            file_id: FILE,
            offset: 0,
            length: 131073,
        }))
        .unwrap();
    let h = SyncHeader::decode(&req).unwrap();
    assert_eq!(
        (h.message_id, h.credit_charge, h.session_id, h.tree_id),
        (4, 3, 9, 7)
    );
    assert_eq!((s.next_message_id(), s.credits()), (7, 3));
    assert!(s
        .build(Request::Read(ReadRequest {
            file_id: FILE,
            offset: 0,
            length: 262144
        }))
        .is_err());
    assert_eq!((s.next_message_id(), s.credits()), (7, 3));
    let mut pending = response(8, 4, 0, 2, &[9, 0, 0, 0, 0, 0, 0, 0, 0]);
    pending[8..12].copy_from_slice(&[3, 1, 0, 0]);
    pending[16] = 3;
    pending[32..40].copy_from_slice(&0x12345678u64.to_le_bytes());
    assert!(s.consume(&pending).unwrap().is_none());
    assert_eq!(s.credits(), 5);
    assert!(s.consume(&pending).is_err());
    assert_eq!(s.credits(), 5);
    let mut final_reply = response(8, 4, 0, 0, READ);
    final_reply[16] = 3;
    final_reply[32..40].copy_from_slice(&0x12345679u64.to_le_bytes());
    sign(&keys.signing, &mut final_reply).unwrap();
    assert!(s.consume(&final_reply).is_err());
    final_reply[32..40].copy_from_slice(&0x12345678u64.to_le_bytes());
    sign(&keys.signing, &mut final_reply).unwrap();
    let r = s.consume(&final_reply).unwrap().unwrap();
    assert_eq!(r.async_id, Some(0x12345678));
    assert_eq!(r.message, final_reply);
    assert_eq!(r.decode(ReadResponse::decode).unwrap().data, b"abc");
    assert_eq!(s.credits(), 5);
    assert!(s.consume(&final_reply).is_err());
    let req = s.build(Request::Flush(FILE)).unwrap();
    assert_eq!(SyncHeader::decode(&req).unwrap().message_id, 7);
}

#[test]
fn bad_signature_cannot_grant_credits_or_connect_a_tree() {
    let (mut s, keys) = authenticated(DialectCap::Default, 0);
    let req = s.build(Request::TreeConnect("\\\\server\\share")).unwrap();
    assert!(verify(&keys.signing, &req));
    let mut good = response(3, 3, 7, 1, TREE);
    sign(&keys.signing, &mut good).unwrap();
    let mut bad = good.clone();
    bad[14] = 99;
    assert!(s.consume(&bad).is_err());
    assert_eq!(s.credits(), 5);
    assert_eq!(s.tree_id(), None);
    s.consume(&good).unwrap().unwrap();
    assert_eq!((s.credits(), s.tree_id()), (6, Some(7)));
}

#[test]
fn encryption_required_by_tree_or_session_wraps_outgoing_requests() {
    for session_encrypt in [false, true] {
        let (mut s, keys) = authenticated(DialectCap::Default, if session_encrypt { 4 } else { 0 });
        let c2s = Transform::new(Cipher::Aes128Gcm, keys.encryption, 9, [0; 3]);
        let mut s2c = Transform::new(Cipher::Aes128Gcm, keys.decryption, 9, [4, 5, 6]);
        if session_encrypt {
            let req = s.build(Request::TreeConnect("\\\\server\\share")).unwrap();
            assert_eq!(&req[..4], b"\xfdSMB");
            assert_eq!(
                SyncHeader::decode(&c2s.decrypt(&req).unwrap())
                    .unwrap()
                    .command,
                3
            );
            let tree = response(3, 3, 7, 1, TREE);
            s.consume(&s2c.encrypt(&tree).unwrap()).unwrap().unwrap();
        } else {
            connect(&mut s, &keys, true);
        }
        for (command, request) in [
            (
                8,
                Request::Read(ReadRequest {
                    file_id: FILE,
                    offset: 0,
                    length: 3,
                }),
            ),
            (
                9,
                Request::Write(WriteRequest {
                    file_id: FILE,
                    offset: 0,
                    data: b"abc",
                }),
            ),
            (7, Request::Flush(FILE)),
        ] {
            let wire = s.build(request).unwrap();
            assert_eq!(&wire[..4], b"\xfdSMB");
            let plain = c2s.decrypt(&wire).unwrap();
            let h = SyncHeader::decode(&plain).unwrap();
            assert_eq!((h.command, h.tree_id, h.session_id), (command, 7, 9));
            assert_eq!(h.flags & 8, 0);
        }
        let mut plain = response(8, 4, 7, 1, READ);
        sign(&keys.signing, &mut plain).unwrap();
        assert!(s.consume(&plain).is_err());
        let encrypted = s2c.encrypt(&plain).unwrap();
        let mut corrupt = encrypted.clone();
        corrupt[4] ^= 1;
        assert!(s.consume(&corrupt).is_err());
        assert_eq!(
            s.consume(&encrypted)
                .unwrap()
                .unwrap()
                .decode(ReadResponse::decode)
                .unwrap()
                .data,
            b"abc"
        );
    }
}

#[test]
fn smb302_validate_mismatch_ends_session_and_match_unlocks_io() {
    for mismatch in [None, Some(112), Some(116), Some(132), Some(134)] {
        let (mut s, keys) = authenticated(DialectCap::Smb302, 0);
        connect(&mut s, &keys, false);
        assert!(s.build(Request::Flush(FILE)).is_err());
        let req = s.build(Request::ValidateNegotiate).unwrap();
        assert!(verify(&keys.signing, &req));
        assert_eq!(
            &req[120..],
            &[
                0x44, 0, 0, 0, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
                0x11, 0x11, 0x11, 0x11, 0x11, 3, 0, 1, 0, 2, 3,
            ]
        );
        let mut reply = response(11, 4, 7, 1, VALIDATE);
        if let Some(at) = mismatch {
            reply[at] ^= 1;
        }
        sign(&keys.signing, &mut reply).unwrap();
        if mismatch.is_some() {
            assert!(s.consume(&reply).is_err());
            assert!(s.is_closed());
            assert!(s.build(Request::Flush(FILE)).is_err());
            assert!(s.build(Request::Negotiate).is_err());
        } else {
            s.consume(&reply).unwrap().unwrap();
            assert!(!s.is_closed());
            s.build(Request::Flush(FILE)).unwrap();
        }
    }
}
