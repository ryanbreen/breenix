use libpixcheck::*;
fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn frame(bytes: &[u8], w: usize, h: usize) -> Frame<'_> {
    Frame::new(bytes, w, h, w * 4, Format::Bgrx).unwrap()
}

#[test]
fn sha256_known_answers_and_every_split() {
    // NIST SHA-256 examples: empty, abc, the two-block message, and a million a's.
    for (bytes, expected) in [
        (
            &b""[..],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            &b"abc"[..],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            &b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"[..],
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ] {
        assert_eq!(hex(sha256(bytes)), expected);
        for split in 0..=bytes.len() {
            let mut hash = Sha256::new();
            hash.update(&bytes[..split]);
            hash.update(&[]);
            hash.update(&bytes[split..]);
            assert_eq!(hex(hash.finish()), expected);
        }
    }
    assert_eq!(
        hex(sha256(&vec![b'a'; 1_000_000])),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}
#[test]
fn canonical_ignores_padding_x_and_channel_order() {
    let bgr = [1, 2, 3, 0, 4, 5, 6, 1, 77, 88, 99, 11];
    let rgb = [3, 2, 1, 6, 5, 4, 9, 9];
    let a = Frame::new(&bgr, 2, 1, 12, Format::Bgrx).unwrap();
    let b = Frame::new(&rgb, 2, 1, 8, Format::Rgb).unwrap();
    assert_eq!(a.canonical(), [1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(a.sha256(), sha256(&[1, 2, 3, 255, 4, 5, 6, 255]));
    assert_eq!(a.sha256(), b.sha256());
    assert!(exact(a, b).unwrap().passed());
}
#[test]
fn exact_detects_last_pixel_and_reports_first_in_scan_order() {
    let a = [0; 32];
    let mut b = a;
    b[29] = 7;
    b[4] = 2;
    let result = exact(frame(&a, 4, 2), frame(&b, 4, 2)).unwrap();
    assert!(!result.passed());
    assert_eq!(result.compared, 8);
    assert_eq!(result.differing, 2);
    assert_eq!(result.max_delta, 7);
    assert_eq!((result.first.unwrap().x, result.first.unwrap().y), (1, 0));
    b[4] = 0;
    let result = exact(frame(&a, 4, 2), frame(&b, 4, 2)).unwrap();
    assert_eq!((result.first.unwrap().x, result.first.unwrap().y), (3, 1));
}
#[test]
fn tolerance_boundary_and_pixel_count_are_independent() {
    let a = vec![0; 8000];
    let mut b = a.clone();
    b[0] = 2;
    b[4] = 2;
    let mut options = Options {
        tolerance: Tolerance {
            channel: 2,
            max_differing: 0,
        },
        reason: "fixture",
        ..Options::default()
    };
    assert!(compare(frame(&a, 2000, 1), frame(&b, 2000, 1), options)
        .unwrap()
        .passed());
    options.tolerance.channel = 1;
    assert!(!compare(frame(&a, 2000, 1), frame(&b, 2000, 1), options)
        .unwrap()
        .passed());
    options.tolerance.max_differing = 2;
    assert!(compare(frame(&a, 2000, 1), frame(&b, 2000, 1), options)
        .unwrap()
        .passed());
    options.tolerance.max_differing = 3;
    assert_eq!(
        compare(frame(&a, 2000, 1), frame(&b, 2000, 1), options),
        Err(Error::Tolerance)
    );
}
#[test]
fn region_masks_count_the_union_and_refuse_vacuity() {
    let a = [0; 64];
    let mut b = a;
    b[0] = 9;
    b[60] = 9;
    let masks = [Mask {
        rect: Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        },
        reason: "one known excluded pixel",
    }];
    let regions = [
        Rect {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        },
        Rect {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
        },
    ];
    let options = Options {
        regions: &regions,
        masks: &masks,
        ..Options::default()
    };
    let result = compare(frame(&a, 4, 4), frame(&b, 4, 4), options).unwrap();
    assert_eq!(result.compared, 15);
    assert_eq!(result.differing, 1);
    let masks = [Mask {
        rect: regions[0],
        reason: "all",
    }];
    assert_eq!(
        compare(
            frame(&a, 4, 4),
            frame(&b, 4, 4),
            Options {
                masks: &masks,
                ..options
            }
        ),
        Err(Error::EmptyDomain)
    );
    let masks = [Mask {
        rect: Rect {
            x: 0,
            y: 0,
            w: 4,
            h: 2,
        },
        reason: "half",
    }];
    assert_eq!(
        compare(
            frame(&a, 4, 4),
            frame(&b, 4, 4),
            Options {
                masks: &masks,
                ..options
            }
        ),
        Err(Error::MaskCoverage)
    );
}
#[test]
fn invalid_geometry_is_never_a_match() {
    assert!(matches!(
        Frame::new(&[], 0, 1, 4, Format::Bgrx),
        Err(Error::Empty)
    ));
    assert!(matches!(
        Frame::new(&[0; 8], 2, 1, 7, Format::Bgrx),
        Err(Error::Stride)
    ));
    assert!(matches!(
        Frame::new(&[0; 7], 2, 1, 8, Format::Bgrx),
        Err(Error::Truncated)
    ));
    assert!(matches!(
        Frame::new(&[], 1, 2, usize::MAX, Format::Bgrx),
        Err(Error::SizeOverflow)
    ));
    assert_eq!(
        exact(frame(&[0; 8], 2, 1), frame(&[0; 8], 1, 2)),
        Err(Error::Geometry)
    );
    let regions = [Rect {
        x: usize::MAX,
        y: 0,
        w: 1,
        h: 1,
    }];
    assert_eq!(
        compare(
            frame(&[0; 4], 1, 1),
            frame(&[0; 4], 1, 1),
            Options {
                regions: &regions,
                ..Options::default()
            }
        ),
        Err(Error::Region)
    );
}
#[test]
fn heatmap_uses_largest_colour_delta_and_normalizes_x() {
    assert_eq!(
        heatmap(
            frame(&[1, 2, 3, 0, 4, 5, 6, 0], 2, 1),
            frame(&[1, 2, 3, 9, 4, 8, 6, 9], 2, 1)
        )
        .unwrap(),
        [0, 0, 0, 255, 0, 252, 255, 255]
    );
}
#[test]
fn thumbnail_partitions_odd_dimensions_without_sampling() {
    let pixels = [
        0, 0, 0, 0, 1, 2, 3, 0, 9, 10, 11, 0, 20, 30, 40, 0, 100, 110, 120, 0, 200, 210, 220, 0,
    ];
    assert_eq!(
        thumbnail(frame(&pixels, 3, 2), 2, 1).unwrap(),
        [10, 15, 20, 255, 78, 83, 89, 255]
    );
    assert_eq!(thumbnail(frame(&pixels, 3, 2), 4, 1), Err(Error::Geometry));
}
