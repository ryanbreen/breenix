//! Literal WM scenario expectations, independent of reported geometry.
use libbui::tree::{Flag, Role, Tree};
use libpixcheck::{Frame, Options, Predicate, Rect};
use serde_json::json;

/// Match a captured front with the generation pinned by a completed composite.
pub fn front_matches(captured: u64, before: u64, after: u64, projected_commit: u64, projected_generation: u64) -> bool {
    before == after && after == projected_generation && captured == projected_commit
}

pub fn desktop(
    tree: &Tree,
    ids: [u64; 2],
    owners: [u64; 2],
    scale: u32,
    screen: [u32; 2],
    top: usize,
) -> Result<(), String> {
    if !matches!(scale, 1 | 2) || top > 1 {
        return Err("WM oracle scale or top outside contract".into());
    }
    let s = i64::from(scale);
    let mismatch = |field| format!("tree-mismatch node=desktop field={field} reported={tree:?}");
    let bounds = [0, 0, i64::from(screen[0]), i64::from(screen[1])];
    if tree.root.key != "desktop"
        || tree.root.role != Role::Canvas
        || tree.root.bounds != bounds
        || tree.root.clip != bounds
        || tree.root.flags != [Flag::Enabled, Flag::Visible]
        || tree.children.len() != 2
    {
        return Err(mismatch("root/bounds/window-count"));
    }
    for (z, index) in [1 - top, top].into_iter().enumerate() {
        let n = &tree.children[z];
        let [x, y] = [[30, 38], [80, 88]][index];
        let bounds = [x * s, y * s, 644 * s, 436 * s];
        let mut flags = vec![Flag::Enabled, Flag::Visible];
        if index == top {
            flags.push(Flag::Focused);
        }
        if n.key != format!("window-{}", ids[index])
            || n.parent.as_deref() != Some("desktop")
            || n.role != Role::Window
            || n.name != ["wm scenario lower", "wm scenario upper"][index]
            || n.bounds != bounds
            || n.clip != bounds
            || n.flags != flags
            || n.value
                != json!({"id":ids[index],"owner":owners[index],"z":z,
                "minimized":false,"chromeless":false})
        {
            return Err(mismatch("title/bounds/z/focus/owner"));
        }
    }
    Ok(())
}

pub fn entry(tree: &Tree, expected: &str, scale: u32) -> Result<(), String> {
    let mut nodes = tree.children.iter().filter(|n| n.key == "entry");
    let node = nodes
        .next()
        .ok_or("tree-mismatch node=entry field=key missing")?;
    let bounds = [20, 42, 240, 28].map(|v| i64::from(v) * i64::from(scale));
    if nodes.next().is_some()
        || node.role != Role::Textfield
        || node.value != json!(expected)
        || node.bounds != bounds
        || node.clip != bounds
        || !node.flags.contains(&Flag::Focused)
        || !node.flags.contains(&Flag::Enabled)
    {
        return Err(format!("tree-mismatch node=entry field=value/bounds/focus expected={expected:?} reported={node:?}"));
    }
    Ok(())
}

pub fn screen(frame: Frame<'_>, scale: u32, top: usize) -> Result<(), String> {
    if !matches!(scale, 1 | 2) || top > 1 {
        return Err("WM screen oracle scale/top".into());
    }
    let s = scale as usize;
    // Exposed client patches, then a shared overlap patch: lower's panel versus
    // upper's resting Submit fill. Coordinates are independent of either tree.
    for (name, [x, y, w, h], color) in [
        ("lower-exposed", [40, 152, 8, 8], 45),
        ("upper-exposed", [700, 480, 8, 8], 45),
        (
            "overlap-owner",
            [364, 166, 16, 4],
            if top == 1 { 60 } else { 45 },
        ),
    ] {
        let regions = [Rect {
            x: x * s,
            y: y * s,
            w: w * s,
            h: h * s,
        }];
        let matched = libpixcheck::structural(
            frame,
            Options {
                regions: &regions,
                ..Default::default()
            },
            Predicate::Uniform {
                region: 0,
                colour: [color; 3],
            },
            None,
        )
        .map_err(|e| format!("WM structural check={name}: {e:?}"))?;
        if !matched {
            return Err(format!(
                "pixel-mismatch check={name} expected=#{color:02x}{color:02x}{color:02x} top={top}"
            ));
        }
    }
    Ok(())
}
