//! A ledger snapshot may omit pending serials even when later outcomes exist.
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libuitest/src/input.rs"]
mod input;
#[path = "../libs/libbui/src/tree.rs"]
mod tree;

#[test]
fn finds_first_missing_script_disposition() {
    let ledger = "41 delivered 9 none\n43 wm-handled 0 none\n44 rejected 0 not-selected\n";
    assert_eq!(input::first_missing_disposition(ledger, &[41, 42, 43, 44]).unwrap(), Some(42));
    assert_eq!(input::first_missing_disposition(ledger, &[41, 43, 44]).unwrap(), None);
    assert_eq!(input::first_missing_disposition(ledger, &[45, 42]).unwrap(), Some(45));
    assert!(input::first_missing_disposition("41 pending 9 none\n", &[41]).is_err());
    assert!(input::first_missing_disposition("broken row\n", &[41]).is_err());
}
