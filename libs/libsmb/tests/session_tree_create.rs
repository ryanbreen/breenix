//! Literal wire fixtures derived from MS-SMB2 sections 2.2.2, 2.2.5–16,
//! 2.2.31/32 and MS-ERREF 2.3. No server capture or protocol implementation.
use libsmb::{msg::*, SyncHeader};

// Fixed synchronous header: one credit, message 7, tree 3, session 9.
const HEADER: [u8; 64] = [
    0xfe, 0x53, 0x4d, 0x42, 64, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];
fn literal(command: u8, status: [u8; 4], response: bool, body: &[u8]) -> Vec<u8> {
    let mut b = HEADER.to_vec();
    b[8..12].copy_from_slice(&status);
    b[12] = command;
    b[16] = u8::from(response);
    b.extend_from_slice(body);
    b
}
fn header() -> SyncHeader {
    SyncHeader::decode(&HEADER).unwrap()
}

const SESSION: &[u8] = &[9, 0, 4, 0, 72, 0, 3, 0, 0x60, 1, 2];
const TREE: &[u8] = &[16, 0, 1, 0, 0x30, 0x80, 0, 0, 8, 0, 0, 0, 0xff, 1, 0x1f, 0];
const CREATE: &[u8] = &[
    89, 0, 0, 1, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0,
    0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0x20, 0, 0, 0, 0,
    0, 0, 0, 0x11, 0, 0, 0, 0, 0, 0, 0, 0x22, 0, 0, 0, 0, 0, 0, 0, 152, 0, 0, 0, 26, 0, 0, 0, 0, 0,
    0, 0, 16, 0, 4, 0, 0, 0, 24, 0, 2, 0, 0, 0, b'T', b'e', b's', b't', 0, 0, 0, 0, 0xaa, 0xbb,
];
const CLOSE: &[u8] = &[
    60, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0,
    0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0x20, 0, 0, 0,
];
const VALIDATE: &[u8] = &[
    49, 0, 0, 0, 4, 2, 0x14, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 112, 0, 0, 0, 0, 0, 0, 0, 112, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x40, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 3, 0, 2, 3,
];
fn create_request<'a>(name: &'a str, contexts: &'a [CreateContext]) -> CreateRequest<'a> {
    CreateRequest {
        name,
        oplock_level: 0,
        impersonation_level: 2,
        desired_access: access_mask::READ_DATA | access_mask::WRITE_DATA | access_mask::DELETE,
        file_attributes: 0x10,
        share_access: 7,
        disposition: CreateDisposition::OpenIf,
        options: create_options::DIRECTORY_FILE | create_options::DELETE_ON_CLOSE,
        contexts,
    }
}

#[test]
fn requests_encode_to_literal_bytes() {
    let h = header();
    assert_eq!(
        SessionSetupRequest {
            flags: 0,
            security_mode: 3,
            capabilities: 1,
            previous_session_id: 0x1234,
            security_buffer: &[0x60, 1, 2]
        }
        .encode(&h)
        .unwrap(),
        literal(
            1,
            [0; 4],
            false,
            &[
                25, 0, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 88, 0, 3, 0, 0x34, 0x12, 0, 0, 0, 0, 0, 0,
                0x60, 1, 2
            ]
        )
    );
    assert_eq!(
        encode_logoff_request(&h).unwrap(),
        literal(2, [0; 4], false, &[4, 0, 0, 0])
    );
    assert_eq!(
        TreeConnectRequest {
            flags: 0,
            path: "\\\\host\\rw"
        }
        .encode(&h)
        .unwrap(),
        literal(
            3,
            [0; 4],
            false,
            &[
                9, 0, 0, 0, 72, 0, 18, 0, 92, 0, 92, 0, 104, 0, 111, 0, 115, 0, 116, 0, 92, 0, 114,
                0, 119, 0
            ]
        )
    );
    assert_eq!(
        encode_tree_disconnect_request(&h).unwrap(),
        literal(4, [0; 4], false, &[4, 0, 0, 0])
    );
    let contexts = [CreateContext {
        name: b"QFid".to_vec(),
        data: vec![],
    }];
    assert_eq!(
        create_request("d\\😀", &contexts).encode(&h).unwrap(),
        literal(
            5,
            [0; 4],
            false,
            &[
                57, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 1,
                0, 16, 0, 0, 0, 7, 0, 0, 0, 3, 0, 0, 0, 1, 16, 0, 0, 120, 0, 8, 0, 128, 0, 0, 0,
                24, 0, 0, 0, 100, 0, 92, 0, 0x3d, 0xd8, 0, 0xde, 0, 0, 0, 0, 16, 0, 4, 0, 0, 0, 0,
                0, 0, 0, 0, 0, b'Q', b'F', b'i', b'd', 0, 0, 0, 0,
            ]
        )
    );
    assert_eq!(
        create_request("", &[]).encode(&h).unwrap(),
        literal(
            5,
            [0; 4],
            false,
            &[
                57, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 1,
                0, 16, 0, 0, 0, 7, 0, 0, 0, 3, 0, 0, 0, 1, 16, 0, 0, 120, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0,
            ]
        )
    );
    assert_eq!(
        CloseRequest {
            file_id: FileId {
                persistent: 0x11,
                volatile: 0x22
            },
            postquery_attributes: true
        }
        .encode(&h)
        .unwrap(),
        literal(
            6,
            [0; 4],
            false,
            &[24, 0, 1, 0, 0, 0, 0, 0, 0x11, 0, 0, 0, 0, 0, 0, 0, 0x22, 0, 0, 0, 0, 0, 0, 0,]
        )
    );
    assert_eq!(
        ValidateNegotiateRequest {
            capabilities: 0x40,
            guid: core::array::from_fn(|i| i as u8),
            security_mode: 3,
            dialects: &[0x0302, 0x0311]
        }
        .encode(&h)
        .unwrap(),
        literal(
            11,
            [0; 4],
            false,
            &[
                57, 0, 0, 0, 4, 2, 0x14, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                255, 255, 255, 255, 255, 120, 0, 0, 0, 28, 0, 0, 0, 0, 0, 0, 0, 120, 0, 0, 0, 0, 0,
                0, 0, 24, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7,
                8, 9, 10, 11, 12, 13, 14, 15, 3, 0, 2, 0, 2, 3, 0x11, 3,
            ]
        )
    );
    // MS-SMB2 disposition constants, used for create/exclusive/truncate mapping.
    assert_eq!(
        [
            CreateDisposition::Supersede as u32,
            CreateDisposition::Open as u32,
            CreateDisposition::Create as u32,
            CreateDisposition::OpenIf as u32,
            CreateDisposition::Overwrite as u32,
            CreateDisposition::OverwriteIf as u32
        ],
        [0, 1, 2, 3, 4, 5]
    );
    assert!(SessionSetupRequest {
        flags: 0,
        security_mode: 1,
        capabilities: 0,
        previous_session_id: 0,
        security_buffer: &vec![0; 65536]
    }
    .encode(&h)
    .is_err());
    assert!(create_request(&"a".repeat(32768), &[]).encode(&h).is_err());
}

#[test]
fn literal_responses_preserve_session_share_and_file_fields() {
    let s = SessionSetupResponse::decode(&literal(1, [0x16, 0, 0, 0xc0], true, SESSION)).unwrap();
    assert_eq!(s.status, NtStatus::MORE_PROCESSING_REQUIRED);
    assert_eq!(s.header.session_id, 9);
    assert_eq!(s.session_flags, session_flags::ENCRYPT_DATA);
    assert_eq!(s.security_buffer, [0x60, 1, 2]);
    assert!(
        SessionSetupResponse::decode(&literal(1, [0; 4], true, &[9, 0, 0, 0, 0, 0, 0, 0]))
            .unwrap()
            .security_buffer
            .is_empty()
    );
    assert_eq!(
        decode_logoff_response(&literal(2, [0; 4], true, &[4, 0, 0, 0]))
            .unwrap()
            .message_id,
        7
    );
    assert_eq!(
        decode_tree_disconnect_response(&literal(4, [0; 4], true, &[4, 0, 0, 0]))
            .unwrap()
            .tree_id,
        3
    );
    let t = TreeConnectResponse::decode(&literal(3, [0; 4], true, TREE)).unwrap();
    assert_eq!(
        (
            t.share_type,
            t.share_flags,
            t.capabilities,
            t.maximal_access
        ),
        (
            1,
            share_flags::ENCRYPT_DATA | share_flags::NO_CACHING,
            8,
            0x1f01ff
        )
    );
    let c = CreateResponse::decode(&literal(5, [0; 4], true, CREATE)).unwrap();
    assert_eq!((c.oplock_level, c.flags, c.create_action), (0, 1, 2));
    assert_eq!(
        c.file_id,
        FileId {
            persistent: 0x11,
            volatile: 0x22
        }
    );
    assert_eq!(
        c.attributes,
        FileAttributes {
            creation_time: 1,
            last_access_time: 2,
            last_write_time: 3,
            change_time: 4,
            allocation_size: 4096,
            end_of_file: 5,
            file_attributes: 0x20
        }
    );
    assert_eq!(
        c.contexts,
        [CreateContext {
            name: b"Test".to_vec(),
            data: vec![0xaa, 0xbb]
        }]
    );
    let close = CloseResponse::decode(&literal(6, [0; 4], true, CLOSE)).unwrap();
    assert_eq!(close.attributes, Some(c.attributes));
    let mut no_attrs = CLOSE.to_vec();
    no_attrs[2] = 0;
    assert_eq!(
        CloseResponse::decode(&literal(6, [0; 4], true, &no_attrs))
            .unwrap()
            .attributes,
        None
    );
    let v = ValidateNegotiateResponse::decode(&literal(11, [0; 4], true, VALIDATE)).unwrap();
    assert_eq!(
        (v.capabilities, v.security_mode, v.dialect),
        (0x40, 3, 0x0302)
    );
    assert_eq!(v.guid, core::array::from_fn(|i| i as u8));
}

#[test]
fn error_responses_preserve_status_and_bound_contexts() {
    let e = ErrorResponse::decode(&literal(
        5,
        [0x22, 0, 0, 0xc0],
        true,
        &[9, 0, 0, 0, 0, 0, 0, 0, 0],
    ))
    .unwrap();
    assert_eq!(e.status, NtStatus::ACCESS_DENIED);
    assert!(e.error_data.is_empty());
    let body = [
        9, 0, 2, 0, 27, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0x11, 3, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0,
        0x53, 0x72, 0x64, 0x72, 1, 2, 3,
    ];
    let e = ErrorResponse::decode(&literal(3, [0xcc, 0, 0, 0xc0], true, &body)).unwrap();
    assert_eq!(
        e.contexts,
        [
            ErrorContext {
                id: 0,
                data: vec![0x11, 3]
            },
            ErrorContext {
                id: 0x72647253,
                data: vec![1, 2, 3]
            }
        ]
    );
    assert_eq!(e.status, NtStatus::BAD_NETWORK_NAME);
    assert_eq!(NtStatus(0xdeadbeef).0, 0xdeadbeef);
    assert_eq!(NtStatus::NETWORK_SESSION_EXPIRED.0, 0xc000035c);
    assert_eq!(NtStatus::NO_MORE_FILES.0, 0x80000006);
    for (at, value) in [(68, 255), (72, 255), (88, 255), (66, 3)] {
        let mut b = literal(3, [0xcc, 0, 0, 0xc0], true, &body);
        b[at] = value;
        assert!(ErrorResponse::decode(&b).is_err(), "error field {at}");
    }
}

#[test]
fn responses_reject_truncation_and_buffers_outside_the_message() {
    type Decoder = fn(&[u8]) -> bool;
    let cases: &[(u8, &[u8], Decoder)] = &[
        (1, SESSION, |b| SessionSetupResponse::decode(b).is_err()),
        (2, &[4, 0, 0, 0], |b| decode_logoff_response(b).is_err()),
        (3, TREE, |b| TreeConnectResponse::decode(b).is_err()),
        (4, &[4, 0, 0, 0], |b| {
            decode_tree_disconnect_response(b).is_err()
        }),
        (5, CREATE, |b| CreateResponse::decode(b).is_err()),
        (6, CLOSE, |b| CloseResponse::decode(b).is_err()),
        (11, VALIDATE, |b| {
            ValidateNegotiateResponse::decode(b).is_err()
        }),
    ];
    for &(cmd, body, rejects) in cases {
        let b = literal(cmd, [0; 4], true, body);
        for n in 0..b.len() {
            assert!(rejects(&b[..n]), "command {cmd} prefix {n}");
        }
        for at in [12, 16, 20, 64] {
            let mut bad = b.clone();
            bad[at] ^= 0x80;
            assert!(rejects(&bad), "command {cmd} header/size {at}");
        }
    }
    // Each offset/length is tested both beyond the end and back into fixed fields.
    for (cmd, body, fields, rejects) in [
        (1, SESSION, &[(68, 2), (70, 2)][..], cases[0].2),
        (
            5,
            CREATE,
            &[
                (144, 4),
                (148, 4),
                (156, 2),
                (158, 2),
                (162, 2),
                (164, 4),
                (152, 4),
            ][..],
            cases[4].2,
        ),
        (
            11,
            VALIDATE,
            &[(88, 4), (92, 4), (96, 4), (100, 4)][..],
            cases[6].2,
        ),
    ] {
        for &(at, width) in fields {
            let mut b = literal(cmd, [0; 4], true, body);
            b[at..at + width].fill(255);
            assert!(rejects(&b), "command {cmd} range {at}");
        }
        let at = match cmd {
            1 => 68,
            5 => 144,
            _ => 96,
        };
        let mut b = literal(cmd, [0; 4], true, body);
        b[at] = 64;
        assert!(rejects(&b), "command {cmd} points into header");
    }
}
