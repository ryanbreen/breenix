use screen_capture::{decode, encode, CaptureLog, Record, HEADER};

fn record(pixels: &[u8]) -> Record<'_> {
    Record { suite: "gpu", case: "draw/one", checkpoint: "end", width: (pixels.len()/4) as u32,
        height: 1, digest: libpixcheck::sha256(pixels), pixels }
}

#[test]
fn scr1_round_trips_and_selects_the_smaller_payload() {
    for (pixels, encoding) in [(vec![1,2,3,255].repeat(128), 1u32),
        ((0..32u8).flat_map(|n| [n,0,0,255]).collect(), 0), (vec![1,2,3,255].repeat(2), 0)] {
        let input = record(&pixels);
        let bytes = encode(&input).unwrap();
        assert_eq!(&bytes[..4], b"SCR1");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), encoding);
        let (output, used) = decode(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(output.pixels, pixels);
        assert_eq!((output.suite.as_str(), output.case.as_str(), output.checkpoint.as_str()),
            ("gpu", "draw/one", "end"));
        assert_eq!((output.width,output.height,output.digest), (input.width,input.height,input.digest));
        assert_eq!(bytes.len() % 512, 0);
    }
}

#[test]
fn scr1_refuses_wrong_digests_lengths_and_damaged_payloads() {
    let pixels = vec![1,2,3,255].repeat(128);
    let mut input = record(&pixels);
    input.digest[0] ^= 1;
    assert!(encode(&input).is_err());
    input = record(&pixels); input.width += 1;
    assert!(encode(&input).is_err());
    let good = encode(&record(&pixels)).unwrap();
    let payload = HEADER + "gpu".len() + "draw/one".len() + "end".len();
    for index in [0, 8, 16, 32, payload, payload+4] {
        let mut bad = good.clone(); bad[index] ^= 1;
        assert!(decode(&bad).is_err(), "accepted changed byte {index}");
    }
    assert!(decode(&good[..good.len()-1]).is_err());
    let mut bad = good.clone(); bad[payload..payload+4].fill(0);
    assert!(decode(&bad).is_err());
    let mut bad = good.clone(); *bad.last_mut().unwrap() = 1;
    assert!(decode(&bad).is_err());
}

#[test]
fn capture_log_writes_only_first_digest_and_never_commits_failed_io() {
    let mut log = CaptureLog::new();
    let mut disk = vec![0; 4096];
    let pixels = vec![1,2,3,255].repeat(128);
    let bytes = encode(&record(&pixels)).unwrap();
    let mut writes = 0;
    let mut write = |offset: u64, data: &[u8]| {
        writes += 1;
        disk[offset as usize..offset as usize+data.len()].copy_from_slice(data); Ok(())
    };
    assert_eq!(log.append(&bytes,4096,&mut write), Ok(true));
    let mut alias = record(&pixels); alias.suite = "breengel"; alias.case = "draw/two";
    assert_eq!(log.append(&encode(&alias).unwrap(),4096,&mut write), Ok(false));
    assert_eq!(writes,1);
    assert_eq!(decode(&disk[512..]).unwrap().0.digest, alias.digest);
    let next = encode(&record(&[4,5,6,255])).unwrap();
    assert!(log.append(&next,4096,|_,_| Err("io")).is_err());
    assert!(log.append(&next,4096,|_,_| panic!("failed disk must stay poisoned")).is_err());
    let mut small = CaptureLog::new();
    assert!(small.append(&bytes,512,|_,_| panic!("out of bounds write")).is_err());
}
