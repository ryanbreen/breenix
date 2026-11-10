use ufs_abi::payload::SetTimes;

#[test]
fn times_use_literal_nanoseconds_and_preserve_omitted_fields() {
    let times = SetTimes { atime_ns: Some(0x0807060504030201), mtime_ns: None };
    let literal = [1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut bytes = [0; SetTimes::SIZE];
    assert_eq!(times.encode(&mut bytes).unwrap(), literal.len());
    assert_eq!(bytes, literal);
    assert_eq!(SetTimes::decode(&literal).unwrap(), times);
    assert!(SetTimes::decode(&literal[..19]).is_err());
    let mut bad = literal;
    bad[0] = 4;
    assert!(SetTimes::decode(&bad).is_err());
}
