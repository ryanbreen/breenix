use cowfs::format::*;

#[test]
fn superblock_offsets_match_doc() {
    let mut s = Superblock::default();
    s.fs_uuid = [0x11; 16];
    s.generation = 5;
    s.ring = 1;
    s.slot = 1;
    s.total_blocks = 0x10101;
    s.compat = 2;
    s.ro_compat = 3;
    s.incompat = 1;
    s.space_root = BlockPtr {
        addr: 7,
        generation: 8,
        csum: 9,
        level: 2,
    };
    s.free_blocks = 10;
    s.global_reserve = 11;
    s.next_root_id = 12;
    s.dirhash_key = [0x22; 16];
    s.commit_time = Time {
        seconds: -1,
        nanos: 13,
    };
    s.mkfs_time = Time {
        seconds: 14,
        nanos: 15,
    };
    s.last_scrub_generation = 16;
    s.last_scrub_errors = 17;
    s.label[0] = b'L';
    s.writer[0] = b'W';
    let b = s.encode();
    assert_eq!(&b[0..8], b"CoWFS-SB");
    assert_eq!(
        u32::from_le_bytes(b[8..12].try_into().unwrap()),
        cowfs::crc32c::checksum(&b[12..])
    );
    assert_eq!(&b[12..16], &1u32.to_le_bytes());
    assert_eq!(&b[16..32], &[0x11; 16]);
    assert_eq!(&b[32..40], &5u64.to_le_bytes());
    assert_eq!(&b[40..44], &[1, 1, 0, 0]);
    assert_eq!(&b[44..48], &4096u32.to_le_bytes());
    assert_eq!(&b[48..52], &16384u32.to_le_bytes());
    assert_eq!(&b[52..56], &[0; 4]);
    for (o, v) in [
        (56, 0x10101u64),
        (64, 2),
        (72, 3),
        (80, 1),
        (88, 7),
        (96, 8),
        (112, 10),
        (120, 11),
        (128, 12),
        (184, 16),
        (192, 17),
    ] {
        assert_eq!(&b[o..o + 8], &v.to_le_bytes());
    }
    assert_eq!(&b[104..108], &9u32.to_le_bytes());
    assert_eq!(&b[108..112], &[2, 0, 0, 0]);
    assert_eq!(&b[136..152], &[0x22; 16]);
    assert_eq!(&b[152..160], &(-1i64).to_le_bytes());
    assert_eq!(&b[160..164], &13u32.to_le_bytes());
    assert_eq!(&b[164..168], &[0; 4]);
    assert_eq!(&b[168..176], &14i64.to_le_bytes());
    assert_eq!(&b[176..180], &15u32.to_le_bytes());
    assert_eq!(&b[180..184], &[0; 4]);
    assert_eq!(b[200], b'L');
    assert!(b[201..264].iter().all(|&x| x == 0));
    assert_eq!(b[264], b'W');
    assert!(b[265..].iter().all(|&x| x == 0));
    assert_eq!(Superblock::decode(&b).unwrap(), s);
    assert!(valid_superblock_slot(&b, 0x10101, 1, 1));
    assert!(!valid_superblock_slot(&b, 0x10100, 1, 1));
    assert!(!valid_superblock_slot(&b, 0x10101, 0, 1));
}

#[test]
fn node_header_offsets_match_doc() {
    let h = NodeHeader {
        tree: 2,
        level: 3,
        nitems: 0x1234,
        fs_uuid: [0x55; 16],
        addr: 123,
        generation: 456,
    };
    let mut b = [0u8; NODE_SIZE];
    b[100] = 0x42;
    h.encode(&mut b).unwrap();
    assert_eq!(&b[0..8], b"CoWFS-ND");
    assert_eq!(
        u32::from_le_bytes(b[8..12].try_into().unwrap()),
        cowfs::crc32c::checksum(&b[12..])
    );
    assert_eq!(&b[12..16], &[2, 3, 0x34, 0x12]);
    assert_eq!(&b[16..32], &[0x55; 16]);
    assert_eq!(&b[32..40], &123u64.to_le_bytes());
    assert_eq!(&b[40..48], &456u64.to_le_bytes());
    assert_eq!(NodeHeader::decode(&b).unwrap(), h);
    b[100] ^= 1;
    assert!(NodeHeader::decode(&b).is_err());
}

#[test]
fn codecs_round_trip() {
    macro_rules! round {
        ($t:ty, $v:expr) => {{
            let v = $v;
            assert_eq!(<$t>::decode(&v.encode()).unwrap(), v);
        }};
    }
    round!(
        BlockPtr,
        BlockPtr {
            addr: 1,
            generation: 2,
            csum: 3,
            level: 4
        }
    );
    let k = Key {
        object: 5,
        item_type: 6,
        offset: 7,
    };
    round!(Key, k);
    assert!(
        k < Key {
            object: 5,
            item_type: 7,
            offset: 0
        }
    );
    round!(
        LeafItemHeader,
        LeafItemHeader {
            key: k,
            data_off: 15000,
            data_len: 20
        }
    );
    round!(
        InternalEntry,
        InternalEntry {
            key: k,
            child_addr: 8,
            child_generation: 9,
            child_csum: 10
        }
    );
    round!(
        Inode,
        Inode {
            mode: 1,
            nlink: 2,
            uid: 3,
            gid: 4,
            size: 5,
            nblocks: 6,
            parent: 7,
            rdev: 8,
            next_index: 9,
            created_generation: 10,
            atime: Time {
                seconds: -1,
                nanos: 11
            },
            mtime: Time {
                seconds: 12,
                nanos: 13
            },
            ctime: Time {
                seconds: 14,
                nanos: 15
            },
            btime: Time {
                seconds: 16,
                nanos: 17
            }
        }
    );
    round!(
        DirRecord,
        DirRecord {
            ino: 3,
            kind: 1,
            name: b"file".to_vec()
        }
    );
    round!(
        Extent,
        Extent {
            phys_start: 3,
            phys_len: 4,
            extent_offset: 1,
            num_blocks: 2,
            flags: 0
        }
    );
    round!(
        Root,
        Root {
            root: BlockPtr {
                addr: 2,
                generation: 3,
                csum: 4,
                level: 0
            },
            next_inode: 16,
            source_root: 0,
            created_generation: 4,
            ctime: Time {
                seconds: -5,
                nanos: 6
            },
            flags: 1,
            name: b"root".to_vec()
        }
    );
    round!(
        Ref,
        Ref {
            refcount: 2,
            kind: 3,
            alloc_generation: 8,
            crcs: vec![1, 2, 3]
        }
    );
    round!(Free, Free { length: 42 });
    round!(
        Pending,
        Pending {
            length: 42,
            released_generation: 7
        }
    );
    round!(
        Xattr,
        Xattr {
            name: b"user.key".to_vec(),
            value: vec![0, 1, 2]
        }
    );
}

#[test]
fn truncated_input_is_an_error() {
    for n in 0..4096 {
        assert!(Superblock::decode(&vec![0; n]).is_err());
    }
    for n in 0..NODE_SIZE {
        assert!(NodeHeader::decode(&vec![0; n]).is_err());
    }
    macro_rules! short {
        ($t:ty, $n:expr) => {
            for n in 0..$n {
                assert!(<$t>::decode(&vec![0; n]).is_err());
            }
        };
    }
    short!(BlockPtr, 24);
    short!(Key, 17);
    short!(LeafItemHeader, 21);
    short!(InternalEntry, 37);
    short!(Inode, 128);
    short!(DirRecord, 10);
    short!(Extent, 24);
    short!(Root, 65);
    short!(Ref, 16);
    short!(Free, 8);
    short!(Pending, 16);
    short!(Xattr, 3);
}
