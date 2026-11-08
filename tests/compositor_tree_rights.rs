#[path = "../kernel/src/window/compositor_tree.rs"]
mod compositor_tree;

use breengel_abi::errno::EPERM;
use compositor_tree::{access, ClaimTree, Access};

#[test]
fn claimant_writes_root_reads_and_others_are_denied() {
    assert_eq!(access(Some(7), 7, false), Ok(Access::Write));
    assert_eq!(access(Some(7), 8, true), Ok(Access::Read));
    assert_eq!(access(Some(7), 8, false), Err(EPERM));
    assert_eq!(access(None, 8, true), Err(EPERM));
}

#[test]
fn a_new_claim_has_fresh_empty_storage() {
    let mut first = ClaimTree::new(7, Vec::<u8>::new(), Vec::<u8>::new());
    first.page.push(42);
    first.readers.push(1);
    let second = ClaimTree::new(8, Vec::<u8>::new(), Vec::<u8>::new());
    assert!(second.page.is_empty());
    assert!(second.readers.is_empty());
    assert_eq!(access(Some(second.holder), 7, false), Err(EPERM));
}
