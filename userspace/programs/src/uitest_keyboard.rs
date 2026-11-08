//! Split key actions from text insertion; bwm sends both for printable keys.
use crate::uitest_widgets::Widgets;
use libbui::{Key, KeyInput};
pub fn press(scene: &mut Widgets, code: u32, shift: bool, ctrl: bool, alt: bool, text: &str) {
    let key = match code {
        15 => Key::Tab,
        28 => Key::Enter,
        1 => Key::Escape,
        14 => Key::Backspace,
        105 => Key::Left,
        106 => Key::Right,
        103 => Key::Up,
        108 => Key::Down,
        102 => Key::Home,
        107 => Key::End,
        111 => Key::Delete,
        104 => Key::PageUp,
        109 => Key::PageDown,
        30 if ctrl => Key::Char,
        57 => Key::Space,
        _ => return,
    };
    // Space inserts via Text in a field, but remains a key action on buttons.
    if code == 57
        && scene
            .tree()
            .children
            .iter()
            .any(|n| n.key == "entry" && n.flags.contains(&libbui::tree::Flag::Focused))
    {
        return;
    }
    let text = if code == 30 && ctrl { "a" } else { text };
    scene.key(&KeyInput {
        key,
        shift,
        ctrl,
        alt,
        text,
    });
}
pub fn text(scene: &mut Widgets, text: &str) {
    // Enter, Tab and Ctrl-letter bytes already arrived as key actions.
    if !text.chars().any(char::is_control) {
        scene.key(&KeyInput::text(text));
    }
}
