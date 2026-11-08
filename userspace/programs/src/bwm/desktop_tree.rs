//! Pure desktop projection of bwm's stack and frame metadata.
use super::stack::Stack;
use breengel::tree::{Flag, Node, Role, Tree};
use serde_json::json;

pub(super) struct WindowState<'a> {
    pub id: u64,
    pub title: &'a str,
    pub owner: u32,
    /// Screen-space frame rectangle, excluding the drop shadow.
    pub bounds: [i64; 4],
    pub chromeless: bool,
}

pub(super) fn build<'a>(stack: &Stack, windows: impl IntoIterator<Item = WindowState<'a>>,
    screen: [i64; 2], scene_version: u64) -> Tree {
    let windows: Vec<_> = windows.into_iter().collect();
    let bounds = [0, 0, screen[0], screen[1]];
    let root = Node { key: "desktop".into(), parent: None, role: Role::Canvas,
        name: "Desktop".into(), value: json!({"scene_version":scene_version}),
        bounds, clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] };
    let children = stack.order().iter().enumerate().filter_map(|(z, id)| {
        let win = windows.iter().find(|win| win.id == *id)?;
        let minimized = stack.minimized(*id);
        let [x,y,w,h] = win.bounds;
        let left = x.max(0);
        let top = y.max(0);
        let clip = if minimized { [left,top,0,0] } else {
            [left,top,(x+w).min(screen[0]).saturating_sub(left).max(0),
                (y+h).min(screen[1]).saturating_sub(top).max(0)]
        };
        let mut flags = vec![Flag::Enabled];
        if clip[2] > 0 && clip[3] > 0 { flags.push(Flag::Visible); }
        if !minimized && stack.focus() == Some(*id) { flags.push(Flag::Focused); }
        Some(Node { key: format!("window-{id}"), parent: Some(root.key.clone()), role: Role::Window,
            name: if win.chromeless { win.title.strip_prefix('\x01').unwrap_or(win.title) } else { win.title }.into(),
            value: json!({"id":id,"owner":win.owner,"z":z,"minimized":minimized,"chromeless":win.chromeless}),
            bounds: win.bounds, clip, flags })
    }).collect();
    Tree { root, children }
}
