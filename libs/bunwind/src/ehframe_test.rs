#[test]
fn corrupt_or_truncated_is_error_not_panic() {
    use crate::cfi::Endian;
    use crate::ehframe::{ErrorKind, Memory, Section, Sections};

    struct NoMemory;
    impl Memory for NoMemory {
        fn read(&self, _: u64, _: usize) -> Option<&[u8]> {
            None
        }
    }

    let frame = include_bytes!("../tests/fixtures/k1-hosted/x86_64.eh_frame");
    let header = include_bytes!("../tests/fixtures/k1-hosted/x86_64.eh_frame_hdr");
    let lookup = |frame, header| {
        Sections {
            frame: Section {
                address: 0x4001_d248,
                bytes: frame,
            },
            header: Some(Section {
                address: 0x4001_d230,
                bytes: header,
            }),
            text_base: None,
            data_base: None,
            address_size: 8,
            endian: Endian::Little,
        }
        .find_fde(&NoMemory, 0x4000_4dcc)
    };

    for len in 0..header.len() {
        assert_eq!(
            lookup(frame, &header[..len]).unwrap_err().kind,
            ErrorKind::Truncated
        );
    }
    // The first FDE ends at byte 48, at the start of the terminator.
    for len in 0..48 {
        assert_eq!(
            lookup(&frame[..len], header).unwrap_err().kind,
            ErrorKind::Truncated
        );
    }

    let mut bad_frame = *frame;
    bad_frame[28] = 0xff; // CIE displacement into bytes before the section
    assert_eq!(
        lookup(&bad_frame, header).unwrap_err().kind,
        ErrorKind::InvalidCiePointer
    );

    let mut bad_frame = *frame;
    bad_frame[16] = 0x1f; // unsupported format in the CIE's R encoding
    assert_eq!(
        lookup(&bad_frame, header).unwrap_err().kind,
        ErrorKind::UnsupportedEncoding(0x1f)
    );

    let mut bad_header = *header;
    bad_header[8..12].fill(0xff); // table count larger than the supplied table
    assert_eq!(
        lookup(frame, &bad_header).unwrap_err().kind,
        ErrorKind::Truncated
    );
}

#[test]
fn lookup_resolves_personality_and_lsda() {
    use crate::cfi::Endian;
    use crate::ehframe::{ErrorKind, FrameSource, Memory, Section, Sections};

    // LSB 5.0 §10.6.1.1.1 and the K2 unit's S and indirect extensions.
    // The personality slot is at 0x3000; the LSDA base is 0x5000.
    const FRAME: &[u8] = &[
        28, 0, 0, 0, 0, 0, 0, 0, // CIE length and ID
        1, b'z', b'P', b'L', b'R', b'S', 0, 1, 0x78,
        16, // version, string, CFI factors/column
        7, 0x9b, 0xec, 0x1f, 0, 0, 0x3a, 0x19, // indirect pcrel P, datarel L, pcrel SLEB R
        0x0c, 7, 8, 0, 0, 0, // CIE instructions
        12, 0, 0, 0, 36, 0, 0, 0, 24, 4, 2, 0xf0, 0xff, 0, 0, 0, // FDE 0x1040..0x1044
        12, 0, 0, 0, 52, 0, 0, 0, 0xc8, 0, 4, 2, 0, 0, 0, 0, // FDE 0x1080..0x1084
    ];
    const HEADER: &[u8] = &[
        1, 0x1b, 0x03, 0x3b, 0xfc, 0xef, 0xff, 0xff, 2, 0, 0, 0, 0x40, 0xf0, 0xff, 0xff, 0x20,
        0xf0, 0xff, 0xff, // PC and FDE relative to 0x2000
        0x80, 0xf0, 0xff, 0xff, 0x30, 0xf0, 0xff, 0xff,
    ];
    const LEB_TABLE_HEADER: &[u8] = &[
        1, 0x1b, 0x01, 0x01, 0xfc, 0xef, 0xff, 0xff, 2, 0xc0, 0x20, 0xa0, 0x20, 0x80, 0x21, 0xb0,
        0x20, // ULEB PCs and FDE addresses
    ];
    const OMITTED_TABLE_HEADER: &[u8] = &[1, 0x1b, 0xff, 0xff, 0xfc, 0xef, 0xff, 0xff];

    struct Image {
        header: &'static [u8],
        personality_slot: bool,
    }
    impl Memory for Image {
        fn read(&self, address: u64, size: usize) -> Option<&[u8]> {
            const PERSONALITY: [u8; 8] = 0x4000_u64.to_le_bytes();
            (self.personality_slot && address == 0x3000 && size == 8).then_some(&PERSONALITY)
        }
    }
    impl FrameSource for Image {
        fn sections_for_pc(&self, _: u64) -> Option<Sections<'_>> {
            Some(Sections {
                frame: Section {
                    address: 0x1000,
                    bytes: FRAME,
                },
                header: Some(Section {
                    address: 0x2000,
                    bytes: self.header,
                }),
                text_base: None,
                data_base: Some(0x5000),
                address_size: 8,
                endian: Endian::Little,
            })
        }
    }

    for header in [HEADER, LEB_TABLE_HEADER, OMITTED_TABLE_HEADER] {
        let image = Image {
            header,
            personality_slot: true,
        };
        let first = crate::ehframe::find_fde(&image, 0x1043).unwrap().unwrap();
        assert_eq!(first.pc_range, 0x1040..0x1044);
        assert_eq!(first.cie.personality, Some(0x4000));
        assert!(first.cie.signal_frame);
        assert_eq!(first.lsda, Some(0x4ff0));
        assert!(crate::ehframe::find_fde(&image, 0x1044).unwrap().is_none());
        let second = crate::ehframe::find_fde(&image, 0x1080).unwrap().unwrap();
        assert_eq!(second.pc_range, 0x1080..0x1084);
        assert_eq!(second.lsda, None);
    }

    let image = Image {
        header: HEADER,
        personality_slot: false,
    };
    assert_eq!(
        crate::ehframe::find_fde(&image, 0x1040).unwrap_err().kind,
        ErrorKind::UnreadablePointer(0x3000),
    );
}

#[test]
fn lsda_relative_bases_and_missing_got() {
    use crate::cfi::Endian;
    use crate::ehframe::{ErrorKind, Memory, Section, Sections};

    struct NoMemory;
    impl Memory for NoMemory {
        fn read(&self, _: u64, _: usize) -> Option<&[u8]> { None }
    }
    // LSB Core 5.0 table 10-6: the LSDA is textrel or datarel.
    let frame = [
        16, 0, 0, 0, 0, 0, 0, 0, 1, b'z', b'L', b'R', 0, 1, 0x78, 16,
        2, 0x2b, 0x1b, 0, 17, 0, 0, 0, 24, 0, 0, 0,
        0xe4, 0, 0, 0, 4, 0, 0, 0, 4, 0x10, 0, 0, 0,
    ];
    let decode = |bytes: &[u8], data_base| {
        Sections {
            frame: Section { address: 0x1000, bytes },
            header: None,
            text_base: Some(0x4000),
            data_base,
            address_size: 8,
            endian: Endian::Little,
        }.find_fde(&NoMemory, 0x1100).map(|fde| fde.unwrap().lsda)
    };
    assert_eq!(decode(&frame, Some(0x5000)).unwrap(), Some(0x4010));
    let mut datarel = frame;
    datarel[17] = 0x3b;
    assert_eq!(decode(&datarel, Some(0x5000)).unwrap(), Some(0x5010));
    let error = decode(&datarel, None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingDataBase);
    assert_eq!(error.offset, 37);
}
