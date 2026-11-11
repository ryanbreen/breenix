//! Wire fixtures from MS-SMB2 2.2.17–22, 2.2.33–40 and MS-FSCC 2.1/2.4.
use libsmb::{msg::*, SyncHeader};

fn header() -> SyncHeader {
    SyncHeader {
        credit_charge: 1,
        status: 0,
        command: 0,
        credits: 1,
        flags: 0,
        next_command: 0,
        message_id: 7,
        tree_id: 3,
        session_id: 9,
        signature: [0; 16],
    }
}
fn packet(command: u16, response: bool, body: &[u8]) -> Vec<u8> {
    let mut h = header();
    h.command = command;
    h.flags = u32::from(response);
    let mut b = h.encode().to_vec();
    b.extend_from_slice(body);
    b
}
fn id() -> FileId {
    FileId {
        persistent: 1,
        volatile: 2,
    }
}

#[test]
fn requests_match_wire_bytes() {
    let h = header();
    assert_eq!(
        ReadRequest {
            file_id: id(),
            offset: 3,
            length: 4
        }
        .encode(&h)
        .unwrap(),
        packet(
            8,
            false,
            &[
                49, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        )
    );
    assert_eq!(
        WriteRequest {
            file_id: id(),
            offset: 3,
            data: b"ab"
        }
        .encode(&h)
        .unwrap(),
        packet(
            9,
            false,
            &[
                49, 0, 112, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'a', b'b'
            ]
        )
    );
    assert_eq!(
        encode_flush_request(&h, id()).unwrap(),
        packet(
            7,
            false,
            &[24, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
        )
    );
    assert_eq!(
        QueryDirectoryRequest {
            file_id: id(),
            pattern: "*",
            output_length: 4096,
            flags: 1
        }
        .encode(&h)
        .unwrap(),
        packet(
            14,
            false,
            &[
                33, 0, 37, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 96, 0, 2,
                0, 0, 16, 0, 0, 42, 0
            ]
        )
    );
    assert_eq!(
        QueryInfoRequest {
            file_id: id(),
            output_length: 4096
        }
        .encode(&h)
        .unwrap(),
        packet(
            16,
            false,
            &[
                41, 0, 1, 18, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
                0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0
            ]
        )
    );
    assert_eq!(
        SetInfoRequest {
            file_id: id(),
            info: SetInfo::EndOfFile(5)
        }
        .encode(&h)
        .unwrap(),
        packet(
            17,
            false,
            &[
                33, 0, 1, 20, 8, 0, 0, 0, 96, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
                0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0
            ]
        )
    );
    assert_eq!(
        SetInfoRequest {
            file_id: id(),
            info: SetInfo::Rename {
                replace_if_exists: true,
                name: "x"
            }
        }
        .encode(&h)
        .unwrap(),
        packet(
            17,
            false,
            &[
                33, 0, 1, 10, 24, 0, 0, 0, 96, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0,
                0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 120,
                0, 0, 0
            ]
        )
    );
    assert_eq!(
        SetInfoRequest {
            file_id: id(),
            info: SetInfo::Disposition(true)
        }
        .encode(&h)
        .unwrap(),
        packet(
            17,
            false,
            &[
                33, 0, 1, 13, 1, 0, 0, 0, 96, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
                0, 0, 0, 0, 0, 1
            ]
        )
    );
    assert_eq!(
        SetInfoRequest {
            file_id: id(),
            info: SetInfo::Basic {
                creation_time: 1,
                last_access_time: 2,
                last_write_time: 3,
                change_time: 4,
                attributes: 32
            }
        }
        .encode(&h)
        .unwrap(),
        packet(
            17,
            false,
            &[
                33, 0, 1, 4, 40, 0, 0, 0, 96, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
                0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0,
                0, 4, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 0
            ]
        )
    );
}

#[test]
fn responses_and_directory_offsets() {
    assert_eq!(
        ReadResponse::decode(&packet(
            8,
            true,
            &[17, 0, 80, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 97, 98]
        ))
        .unwrap()
        .data,
        b"ab"
    );
    assert_eq!(
        WriteResponse::decode(&packet(
            9,
            true,
            &[17, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        ))
        .unwrap()
        .count,
        2
    );
    assert!(decode_flush_response(&packet(7, true, &[4, 0, 0, 0])).is_ok());
    assert!(decode_set_info_response(&packet(17, true, &[2, 0])).is_ok());
    let mut entries = vec![0u8; 218];
    entries[0..4].copy_from_slice(&112u32.to_le_bytes());
    entries[60..64].copy_from_slice(&2u32.to_le_bytes());
    entries[112 + 60..112 + 64].copy_from_slice(&2u32.to_le_bytes());
    entries[104] = b'a';
    entries[112 + 104] = b'b';
    let parsed = decode_directory_entries(&entries).unwrap();
    assert_eq!(
        parsed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    entries[0..4].copy_from_slice(&4u32.to_le_bytes());
    assert!(decode_directory_entries(&entries).is_err());
    entries[0..4].copy_from_slice(&224u32.to_le_bytes());
    assert!(decode_directory_entries(&entries).is_err());
    let mut info = vec![0; 102];
    info[48..56].copy_from_slice(&5u64.to_le_bytes());
    info[96..100].copy_from_slice(&2u32.to_le_bytes());
    info[100] = b'x';
    let all = decode_file_all_information(&info).unwrap();
    assert_eq!((all.end_of_file, all.name.as_str()), (5, "x"));
    let mut body = vec![9, 0, 72, 0, 102, 0, 0, 0];
    body.extend_from_slice(&info);
    assert_eq!(
        QueryInfoResponse::decode(&packet(16, true, &body))
            .unwrap()
            .info,
        all
    );
    let mut body = vec![9, 0, 72, 0, 218, 0, 0, 0];
    entries[0..4].copy_from_slice(&112u32.to_le_bytes());
    body.extend_from_slice(&entries);
    assert_eq!(
        QueryDirectoryResponse::decode(&packet(14, true, &body))
            .unwrap()
            .entries
            .len(),
        2
    );
}

#[test]
fn credits_and_chunks() {
    assert_eq!(
        [credit_charge(1), credit_charge(65536), credit_charge(65537)],
        [1, 1, 2]
    );
    assert_eq!(plan_chunks(150000, 70000).unwrap(), [70000, 70000, 10000]);
    assert!(plan_chunks(1, 0).is_err());
}

#[test]
fn filesystem_full_size_wire_vector() {
    let request = QueryFsSizeRequest { file_id: id() }
        .encode(&header())
        .unwrap();
    assert_eq!(
        request,
        packet(
            16,
            false,
            &[
                41, 0, 2, 7, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
                0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
            ]
        )
    );
    let response = packet(
        16,
        true,
        &[
            9, 0, 72, 0, 32, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 50, 0, 0,
            0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 2, 0, 0,
        ],
    );
    assert_eq!(
        QueryFsSizeResponse::decode(&response).unwrap().info,
        FileFsFullSizeInformation {
            total_allocation_units: 100,
            caller_available_allocation_units: 40,
            actual_available_allocation_units: 50,
            sectors_per_allocation_unit: 8,
            bytes_per_sector: 512,
        }
    );
    assert!(QueryFsSizeResponse::decode(&response[..response.len() - 1]).is_err());
}
