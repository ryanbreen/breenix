#[path = "../kernel/src/gpu/completion.rs"]
mod completion;

use completion::finish_write;

#[test]
fn partial_success_cannot_publish_failed_contents() {
    let (mut invalid, mut failed_at) = (false, 0);
    finish_write(&mut invalid, &mut failed_at, 10, false, true);
    finish_write(&mut invalid, &mut failed_at, 11, true, false);
    assert!(invalid);
    assert_eq!(failed_at, 10);
    finish_write(&mut invalid, &mut failed_at, 12, true, true);
    assert!(!invalid);
}

#[test]
fn older_whole_write_cannot_recover_newer_failure() {
    let (mut invalid, mut failed_at) = (false, 0);
    finish_write(&mut invalid, &mut failed_at, 12, false, false);
    finish_write(&mut invalid, &mut failed_at, 10, false, false);
    finish_write(&mut invalid, &mut failed_at, 11, true, true);
    assert!(invalid);
    assert_eq!(failed_at, 12);
    finish_write(&mut invalid, &mut failed_at, 13, true, false);
    assert!(invalid);
    finish_write(&mut invalid, &mut failed_at, 14, true, true);
    assert!(!invalid);
    finish_write(&mut invalid, &mut failed_at, 15, false, true);
    assert!(invalid);
}

#[test]
fn partial_write_keeps_a_valid_target_valid() {
    let (mut invalid, mut failed_at) = (false, 0);
    finish_write(&mut invalid, &mut failed_at, 1, true, false);
    assert!(!invalid);
    assert_eq!(failed_at, 0);
}
