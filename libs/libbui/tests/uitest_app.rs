//! Exercise the app's actual routing and the guest's literal tree oracles off hardware.
#[path = "../../../userspace/programs/src/uitest_suite/tree.rs"]
mod checks;
#[path = "../../../userspace/programs/src/uitest_widget_layout.rs"]
mod uitest_widget_layout;
#[path = "../../../userspace/programs/src/uitest_widgets.rs"]
mod widgets;
use libbui::{InputState, Key, KeyInput};
use widgets::Widgets;
fn click(scene: &mut Widgets, x: i32, y: i32) {
    scene.pointer(&InputState::from_raw(x, y, 1, 0, 0));
    scene.pointer(&InputState::from_raw(x, y, 0, 1, 0));
}
#[test]
fn scene_matches_guest_literals_and_rejects_wrong_tree_fields() {
    let scene = Widgets::new();
    let tree = scene.tree();
    checks::expect_roles_keys(&tree, 1).unwrap();
    checks::expect_names_values(&tree, false).unwrap();
    checks::expect_enabled(&tree).unwrap();
    checks::expect_hidden(&tree, 0, 1).unwrap();
    checks::expect_terminal(&tree, 1).unwrap();
    // Fit the window's default semantic page without losing a node.
    let mut page = vec![0; 16 * 1024];
    tree.write(&mut page).unwrap();
    assert_eq!(libbui::tree::Tree::parse(&page).unwrap(), tree);
    let mut bad = tree.clone();
    bad.children[0].role = libbui::tree::Role::Button;
    assert!(checks::expect_roles_keys(&bad, 1).is_err());
    let mut bad = tree.clone();
    bad.children[0].value = "wrong".into();
    assert!(checks::expect_names_values(&bad, false).is_err());
    let mut bad = tree.clone();
    bad.children
        .iter_mut()
        .find(|n| n.key == "disabled")
        .unwrap()
        .flags
        .push(libbui::tree::Flag::Enabled);
    assert!(checks::expect_enabled(&bad).is_err());
    let mut bad = tree.clone();
    bad.children
        .iter_mut()
        .find(|n| n.key == "list/item-19")
        .unwrap()
        .flags
        .push(libbui::tree::Flag::Visible);
    assert!(checks::expect_hidden(&bad, 0, 1).is_err());
    let mut bad = tree;
    bad.children
        .iter_mut()
        .find(|n| n.key == "terminal")
        .unwrap()
        .value["cursor"] = serde_json::json!([0, 0]);
    assert!(checks::expect_terminal(&bad, 1).is_err());
}
#[test]
fn real_widget_routing_produces_scripted_names_values_and_scroll_clips() {
    let mut scene = Widgets::new();
    click(&mut scene, 30, 55);
    scene.key(&KeyInput::text("hi"));
    click(&mut scene, 25, 100);
    click(&mut scene, 340, 102);
    click(&mut scene, 30, 180);
    click(&mut scene, 485, 162);
    click(&mut scene, 25, 10);
    click(&mut scene, 30, 33);
    click(&mut scene, 330, 55);
    checks::expect_names_values(&scene.tree(), true).unwrap();
    // Disabled button does not submit; scrolling changes the same keyed nodes.
    click(&mut scene, 450, 55);
    checks::expect_names_values(&scene.tree(), true).unwrap();
    scene.pointer(&InputState::from_raw(30, 180, 0, 0, 0));
    scene.scroll(-1);
    checks::expect_hidden(&scene.tree(), 40, 1).unwrap();
}
#[test]
fn capture_and_focus_route_to_the_pressed_or_focused_widget() {
    let mut scene = Widgets::new();
    scene.pointer(&InputState::from_raw(340, 102, 1, 0, 0));
    scene.pointer(&InputState::from_raw(600, 55, 1, 1, 0));
    assert_eq!(checks::node(&scene.tree(), "amount").unwrap().value, 100.0);
    scene.pointer(&InputState::from_raw(330, 55, 0, 1, 0));
    assert_eq!(scene.tree().root.value["submissions"], 0);
    assert!(!checks::node(&scene.tree(), "amount")
        .unwrap()
        .flags
        .contains(&libbui::tree::Flag::Pressed));
    click(&mut scene, 30, 55);
    scene.key(&KeyInput::text("λ"));
    scene.key(&KeyInput::new(Key::Tab)); // submit
    scene.key(&KeyInput::new(Key::Tab)); // skip disabled, reach checkbox
    scene.key(&KeyInput::new(Key::Space));
    assert_eq!(checks::node(&scene.tree(), "entry").unwrap().value, "λ");
    assert_eq!(checks::node(&scene.tree(), "accept").unwrap().value, true);
    assert!(checks::node(&scene.tree(), "accept")
        .unwrap()
        .flags
        .contains(&libbui::tree::Flag::Focused));
    scene.pointer(&InputState::from_raw(340, 102, 1, 0, 0));
    scene.cancel_pointer();
    assert!(!checks::node(&scene.tree(), "amount")
        .unwrap()
        .flags
        .contains(&libbui::tree::Flag::Pressed));
}

#[test]
fn committed_geometry_matches_render_scale_on_both_display_scales() {
    let scene = Widgets::new();
    for scale in [1, 2] {
        let tree = scene.committed_tree(scale);
        checks::expect_roles_keys(&tree, scale).unwrap();
        checks::expect_hidden(&tree, 0, scale).unwrap();
        checks::expect_terminal(&tree, scale).unwrap();
        // A logical tree on a scale-2 target must be rejected.
        if scale == 2 {
            assert!(checks::expect_roles_keys(&scene.tree(), scale).is_err());
        }
    }
}
