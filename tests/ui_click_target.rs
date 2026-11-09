extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../libs/libuitest/src/input.rs"]
pub mod input;
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

#[test]
fn reader_probe_reaches_content_instead_of_resize_chrome() {
    let content = [64, 136, 1280, 800];
    let screen = [0, 0, 2560, 1600];
    let clear = input::tree_probe_point(content, screen, &[]).unwrap();
    assert_eq!(clear, (704, 536));
    let exposed = input::tree_probe_point(content, screen, &[[108, 156, 1288, 872]]).unwrap();
    assert_eq!(exposed, (86, 546));
    assert!(input::tree_probe_point(content, screen, &[[0, 0, 2560, 1600]]).is_err());

    // Execute bwm's resize policy with its production constants at scale 2.
    let desk = include_str!("../userspace/programs/src/bwm/desk.rs");
    let start = desk.find("    pub(super) fn hit_resize_edge(").unwrap();
    let end = start + desk[start..].find("\n    }\n").unwrap() + 6;
    let method = desk[start..end].replace("pub(super) fn", "fn");
    let config = include_str!("../userspace/programs/src/bwm/config.rs");
    let constant = |name: &str| -> usize {
        config.lines().find_map(|line| line.strip_prefix(&format!("pub(super) const {name}: usize = ")))
            .unwrap().trim_end_matches(';').parse().unwrap()
    };
    let border = constant("BORDER_WIDTH") * 2;
    let title = constant("TITLE_BAR_HEIGHT") * 2;
    let grab = constant("RESIZE_GRAB");
    let fixture = format!(r#"
const RESIZE_GRAB: usize = {grab};
fn px(n: usize) -> usize {{ n * 2 }}
#[derive(Debug, PartialEq)]
enum ResizeEdge {{ TopLeft, TopRight, BottomLeft, BottomRight, Left, Right, Top, Bottom }}
struct Window {{ x: i32, y: i32, width: usize, height: usize, chromeless: bool }}
impl Window {{ fn total_height(&self) -> usize {{ self.height }} {method} }}
fn main() {{
    let win = Window {{ x: {}, y: {}, width: {}, height: {}, chromeless: false }};
    assert_eq!(win.hit_resize_edge(74, 166), Some(ResizeEdge::Left));
    assert_eq!(win.hit_resize_edge({}, {}), None);
    assert_eq!(win.hit_resize_edge({}, {}), None);
}}
"#, 64 - border as i32, 136 - title as i32 - border as i32,
        1280 + border * 2, 800 + title + border * 2,
        clear.0, clear.1, exposed.0, exposed.1);
    let dir = std::env::temp_dir().join(format!("ui-reader-probe-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let source = dir.join("probe.rs");
    let binary = dir.join("probe");
    std::fs::write(&source, fixture).unwrap();
    let built = std::process::Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(&source).arg("-o").arg(&binary).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&binary).output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let live = include_str!("../libs/libuitest/src/pointer_live.rs");
    assert!(live.contains("tree_probe_point("), "live attachment must use the tested point");
}
