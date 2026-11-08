extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../libs/libuitest/src/input.rs"]
mod input;
use input::{click_target, home_then_move};
use tree::{Flag, Node, Role};

fn node() -> Node {
    Node { key: "submit".into(), parent: Some("root".into()), role: Role::Button,
        name: "Submit".into(), value: serde_json::Value::Null,
        bounds: [10, 20, 100, 40], clip: [0, 0, 640, 480],
        flags: vec![Flag::Enabled, Flag::Visible] }
}

#[test]
fn covered_and_zero_area_fail_by_name() {
    assert!(click_target(&node(), (100, 200), &[]).unwrap_err().contains("submit"));
    for bounds in [[10, 20, 0, 40], [10, 20, 100, 0], [10, 20, -1, 40]] {
        let mut n = node(); n.bounds = bounds;
        assert!(click_target(&n, (100, 200), &[[0, 0, 1000, 1000]]).unwrap_err().contains("submit"));
    }
    let mut hidden = node(); hidden.flags.retain(|f| *f != Flag::Visible);
    assert!(click_target(&hidden, (0, 0), &[[0, 0, 640, 480]]).is_err());
}

#[test]
fn clipped_and_partly_covered_choose_largest_exposed_rectangle() {
    let mut n = node();
    assert_eq!(click_target(&n, (100, 200), &[[100, 200, 640, 480]]).unwrap(), (160, 240));
    // Visible region is in screen pixels; node bounds and clip are window-local.
    n.clip = [20, 30, 80, 20];
    let visible = [[100, 200, 40, 480], [180, 200, 20, 480]];
    assert_eq!(click_target(&n, (100, 200), &visible).unwrap(), (130, 240));
    assert_eq!(click_target(&n, (100, 200), &[visible[1], visible[0]]).unwrap(), (130, 240));
    assert_eq!(click_target(&n, (100, 200), &[[145, 235, 31, 7]]).unwrap(), (160, 238));
    n.clip = [500, 400, 10, 10];
    assert!(click_target(&n, (100, 200), &visible).is_err());
    n.bounds = [-10, -10, 20, 20]; n.clip = n.bounds;
    assert_eq!(click_target(&n, (0, 0), &[[0, 0, 640, 480]]).unwrap(), (5, 5));
    n.bounds = [i64::MAX - 4, 0, 100, 40]; n.clip = n.bounds;
    assert!(click_target(&n, (100, 0), &[[0, 0, 640, 480]]).is_err());
}

#[test]
fn homing_matches_bwm_production_clamp() {
    // Compile the production batch function, avoiding unrelated router dependencies.
    let source = include_str!("../userspace/programs/src/bwm/pointer_route.rs");
    let (_, batch) = source.split_once("#[derive(Clone, Copy)]\npub(super) enum Axis").unwrap();
    let mut fixture = format!("mod route {{ #[derive(Clone, Copy)] pub(super) enum Axis{batch} }}\nfn main() {{\n");
    for screen in [(1, 1), (1280, 800), (2560, 1600), (i32::MAX as usize, 2)] {
        for target in [(0, 0), (screen.0 as i32 - 1, screen.1 as i32 - 1), (17.min(screen.0 as i32 - 1), 23.min(screen.1 as i32 - 1))] {
            let moves = home_then_move(screen, target).unwrap();
            assert_eq!(moves, [(-(screen.0 as i32), -(screen.1 as i32)), target]);
            for start in [(0, 0), (screen.0 as i32 - 1, screen.1 as i32 - 1), (screen.0 as i32 / 2, screen.1 as i32 / 2)] {
                fixture.push_str(&format!(
                    "assert_eq!(route::batch_positions(&[(1,route::Axis::RelativeX({})),(1,route::Axis::RelativeY({})),(1,route::Axis::Boundary),(1,route::Axis::RelativeX({})),(1,route::Axis::RelativeY({}))], {start:?}, {screen:?}), vec![Some((0,0)),Some((0,0)),None,Some({target:?}),Some({target:?})]);\n",
                    moves[0].0, moves[0].1, moves[1].0, moves[1].1));
            }
        }
    }
    fixture.push_str("assert_eq!(route::batch_positions(&[(1,route::Axis::AbsoluteX(99)),(1,route::Axis::AbsoluteY(99))], (0,0), (10,10)), vec![Some((9,9));2]); }");
    let temp = std::env::temp_dir().join(format!("ui-click-clamp-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let src = temp.join("clamp.rs"); let exe = temp.join("clamp");
    std::fs::write(&src, fixture).unwrap();
    let built = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings"]).arg(&src).arg("-o").arg(&exe).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&exe).output().unwrap();
    std::fs::remove_dir_all(&temp).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    for (screen, target) in [((0, 1), (0, 0)), ((20, 10), (-1, 2)), ((20, 10), (20, 2)), ((20, 10), (2, 10))] {
        assert!(home_then_move(screen, target).is_err());
    }
}
