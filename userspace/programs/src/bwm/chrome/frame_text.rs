use libbui::tree::{Role, Tree};

pub fn cursor_row(tree: &Tree) -> Option<(usize, &str)> {
    if tree.root.role != Role::Terminal {
        return None;
    }
    let value = &tree.root.value;
    let cursor = value.get("cursor")?.as_array()?;
    if cursor.len() != 2 {
        return None;
    }
    cursor[0].as_u64()?;
    let row = usize::try_from(cursor[1].as_u64()?).ok()?;
    let text = value.get("lines")?.as_array()?.get(row)?.as_str()?;
    Some((row, text))
}
