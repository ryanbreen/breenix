use breengel_abi::tc_point as w;
use gpu_abi::test_point as g;

#[test]
fn launch_point_extends_the_disjoint_point_allocations() {
    let window = [w::IGNORE_DAMAGE,w::IRQ_HOLD,w::EMIT_DIAGNOSTIC,w::ACQUIRE_STALL,w::BACKEND_ERROR,w::SKIP_TRANSFER,w::WRONG_RESOURCE,w::STALE_FENCE,w::CORRUPT_OUTSIDE_DAMAGE,w::PANEL_CURSOR_NO_RESTORE,w::RELEASE_STALL,w::STAGE_LIE,w::STAGE_SUBSTITUTE_FRAME,w::STAGE_DRAW_IN_WINDOW,w::RIBBON_DROP_STAGE,w::COMPOSITE_STALL,w::MOVE_POINTER,w::CAPTURE_STALL,w::INPUT_REPORT_SKIPPED,w::LAUNCH_FORK_FAIL];
    let gpu = [g::SKIP_TRANSFER,g::STALE_FENCE,g::WRONG_RESOURCE,g::CORRUPT_PIXEL,g::MASK_CAPABILITY,g::PROFILE_VIOLATION,g::SKIP_PRESENT_FLUSH,g::WRONG_SCANOUT,g::DEVICE_ERROR,g::SUPPRESS_DRAW,g::WRONG_TEXTURE,g::SKIP_UPLOAD_VISIBILITY,g::WITHHOLD_FENCE,g::CRED_ORPHAN];
    assert_eq!(window, [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,33,34]);
    assert_eq!(gpu, [19,20,21,22,23,24,25,26,27,28,29,30,31,32]);
    let mut all = window.to_vec(); all.extend(gpu); all.sort_unstable(); all.dedup();
    assert_eq!(all, (1..=34).collect::<Vec<_>>());
    assert_eq!(w::LAST, 34);
    assert!(w::defined(34));
    for p in 1..=33 { assert_eq!(w::defined(p), p <= 18 || p == 33); }
    assert!(!w::defined(35));
}
