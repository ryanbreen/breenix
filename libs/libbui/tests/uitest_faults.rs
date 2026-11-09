//! Exercise the app's actual state and drawing selection without a GPU.
#[path = "../../../userspace/programs/src/uitest_widget_layout.rs"]
mod uitest_widget_layout;
#[path = "../../../userspace/programs/src/uitest_widgets.rs"]
mod widgets;
use widgets::Widgets;
use libbui::{KeyInput, Theme};
fn entry(scene: &Widgets) -> serde_json::Value {
    scene.tree().children.into_iter().find(|n|n.key=="entry").unwrap().value
}
#[test]
fn each_fault_changes_only_its_named_behavior() {
    for (color, drop_key, stale) in [(false,false,false),(true,false,false),(false,true,false),(false,false,true)] {
        let mut scene = Widgets::faulted(color, drop_key, stale);
        let initial = scene.tree();
        assert_eq!(initial, Widgets::new().tree());
        scene.key(&KeyInput::text("a"));
        scene.key(&KeyInput::text("bλ"));
        let expected = if drop_key {"bλ"} else {"abλ"};
        assert_eq!(entry(&scene), expected);
        let mut clean = Widgets::new();
        clean.key(&KeyInput::text(expected));
        assert_eq!(scene.tree(), clean.tree());
        assert_eq!(scene.drawing_scene().tree(), if stale {initial} else {scene.tree()});
        let mut theme = Theme::dark();
        if color { theme.widget_bg = libgfx::color::Color::rgb(0xd0,0x20,0x40); }
        assert_eq!(format!("{:?}",scene.submit_theme()),format!("{theme:?}"));
    }
}
#[test]
fn drop_one_unicode_scalar_then_preserve_navigation_and_later_text() {
    let mut scene=Widgets::faulted(false,true,false);
    scene.key(&KeyInput::text("λx"));
    scene.key(&KeyInput::text("y"));
    assert_eq!(entry(&scene), "xy");
    scene.key(&KeyInput::new(libbui::Key::Backspace));
    assert_eq!(entry(&scene), "x");
}
