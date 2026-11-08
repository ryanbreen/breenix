use libbreenix::input::input_device_selection;

#[test]
fn selects_all_when_unset_and_exact_ids_when_set() {
    assert_eq!(input_device_selection(None).unwrap(), None);
    assert_eq!(input_device_selection(Some("1,42,4294967295")).unwrap(), Some(vec![1, 42, u32::MAX]));
}

#[test]
fn rejects_invalid_device_lists() {
    for value in ["", "0", "1,0", "1,1", "1,01", ",1", "1,", "1,,2", "-1", "+1", " 1", "1x", "4294967296"] {
        assert!(input_device_selection(Some(value)).is_err(), "accepted {value:?}");
    }
}
