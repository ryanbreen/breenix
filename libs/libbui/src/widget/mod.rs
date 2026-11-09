pub mod button;
pub mod checkbox;
pub mod file_picker;
pub mod label;
pub mod list;
pub mod menu_bar;
pub mod panel;
pub mod scroll_bar;
pub mod slider;
pub mod tab_bar;
pub mod text_field;

use libgfx::color::Color;
use libgfx::surface::Surface as FrameBuf;
use crate::drawing as shapes;

use crate::input::InputState;
use crate::rect::Rect;
use crate::theme::Theme;

/// Optional trait for uniform widget handling.
pub trait Widget {
    type Event;
    fn rect(&self) -> Rect;
    fn draw(&self, fb: &mut FrameBuf, theme: &Theme);
    fn update(&mut self, input: &InputState) -> Self::Event;

    /// Report this widget using the caller's stable key and effective ancestor clip.
    /// Custom widgets default to a canvas and can override their semantic state.
    fn tree_node(&self, key: &str, parent: Option<&str>, clip: Rect) -> crate::tree::Node {
        node(key, parent, crate::tree::Role::Canvas, "", self.rect(), clip)
    }

    /// Preorder nodes, including a compound widget's semantic children.
    fn tree_nodes(&self, key: &str, parent: Option<&str>, clip: Rect, _theme: &Theme) -> alloc::vec::Vec<crate::tree::Node> {
        alloc::vec![self.tree_node(key, parent, clip)]
    }
}

pub(crate) fn name(bytes: &[u8]) -> alloc::string::String {
    alloc::string::String::from_utf8_lossy(bytes).into_owned()
}

pub(crate) fn node(key: &str, parent: Option<&str>, role: crate::tree::Role, name: &str, bounds: Rect, ancestor_clip: Rect) -> crate::tree::Node {
    use crate::tree::Flag;
    let clip = bounds.intersection(ancestor_clip);
    let mut flags = alloc::vec![Flag::Enabled];
    if clip.w > 0 && clip.h > 0 { flags.push(Flag::Visible); }
    crate::tree::Node {
        key: key.into(), parent: parent.map(Into::into), role, name: name.into(),
        value: serde_json::Value::Null, bounds: bounds.as_tree_rect(), clip: clip.as_tree_rect(), flags,
    }
}

pub(crate) fn flag(node: &mut crate::tree::Node, flag: crate::tree::Flag, set: bool) {
    node.flags.retain(|f| *f != flag);
    if set { node.flags.push(flag); }
}

/// Draw a filled background for a widget, selecting color based on hover/pressed state.
pub fn draw_widget_bg(fb: &mut FrameBuf, rect: &Rect, theme: &Theme, hovered: bool, pressed: bool) {
    let bg = if pressed {
        theme.widget_bg_active
    } else if hovered {
        theme.widget_bg_hover
    } else {
        theme.widget_bg
    };
    shapes::fill_rect(fb, rect.x, rect.y, rect.w, rect.h, bg);
}

/// Draw a border around a widget rectangle.
pub fn draw_widget_border(fb: &mut FrameBuf, rect: &Rect, color: Color) {
    shapes::draw_rect(fb, rect.x, rect.y, rect.w, rect.h, color);
}

/// Draw the contract focus perimeter, two pixels outside the widget bounds.
pub fn draw_focus_ring(fb: &mut FrameBuf, rect: &Rect, theme: &Theme) {
    shapes::draw_rect(
        fb,
        rect.x - 2,
        rect.y - 2,
        rect.w + 4,
        rect.h + 4,
        theme.focus_ring,
    );
}
