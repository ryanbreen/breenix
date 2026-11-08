//! A ledger snapshot may omit pending serials even when later outcomes exist.
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libuitest/src/input.rs"]
pub mod input;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;

#[test]
fn finds_first_missing_script_disposition() {
    let ledger = "41 delivered 9 none\n43 wm-handled 0 none\n44 rejected 0 not-selected\n";
    assert_eq!(
        input::first_missing_disposition(ledger, &[41, 42, 43, 44]).unwrap(),
        Some(42)
    );
    assert_eq!(
        input::first_missing_disposition(ledger, &[41, 43, 44]).unwrap(),
        None
    );
    assert_eq!(
        input::first_missing_disposition(ledger, &[45, 42]).unwrap(),
        Some(45)
    );
    assert!(input::first_missing_disposition("41 pending 9 none\n", &[41]).is_err());
    assert!(input::first_missing_disposition("broken row\n", &[41]).is_err());
}

#[test]
fn delivery_requires_a_committed_source_watermark() {
    assert!(!input::delivery_ready(0, 41, 41));
    assert!(!input::delivery_ready(9, 40, 41));
    assert!(input::delivery_ready(9, 41, 41));
    assert!(input::delivery_ready(9, 44, 41));
    let old_derived = libbreenix::input::abi::derived_serial(40, 1).unwrap();
    assert!(!input::delivery_ready(9, old_derived, 41));
    assert!(input::delivery_ready(
        9,
        libbreenix::input::abi::derived_serial(41, 1).unwrap(),
        41
    ));
}

#[test]
fn rejects_duplicate_or_malformed_terminal_rows() {
    for ledger in [
        "41 delivered 9 none\n41 rejected 0 not-selected\n",
        "0 delivered 9 none\n",
        "41 delivered wrong none\n",
        "41 delivered 9 none extra\n",
    ] {
        assert!(
            input::first_missing_disposition(ledger, &[41]).is_err(),
            "{ledger}"
        );
    }
    assert_eq!(
        input::first_missing_disposition("41 lost 0 overflow\n", &[41]).unwrap(),
        None
    );
}
