//! Scenario expectations are literals, independent of the app's published values.
use libbui::tree::{Flag, Tree};
use serde_json::{json, Value};

fn node<'a>(tree: &'a Tree, key: &str) -> Result<&'a libbui::tree::Node, String> {
    let mut nodes = std::iter::once(&tree.root)
        .chain(&tree.children)
        .filter(|n| n.key == key);
    let found = nodes
        .next()
        .ok_or_else(|| format!("tree-mismatch node={key} field=key missing"))?;
    if nodes.next().is_some() {
        return Err(format!("tree-mismatch node={key} field=key duplicate"));
    }
    Ok(found)
}
fn equal(key: &str, field: &str, got: Value, want: Value) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "tree-mismatch node={key} field={field} expected={want} got={got}"
        ))
    }
}
pub fn form(tree: &Tree, stage: usize, scale: u32) -> Result<(), String> {
    if stage > 4 {
        return Err("unknown form stage".into());
    }
    let focused = ["entry", "entry", "accept", "amount", "submit"][stage];
    for (key, bounds, value) in [
        (
            "root",
            [0, 0, 640, 400],
            json!({"submissions":u64::from(stage == 4),"menu_action":0}),
        ),
        (
            "entry",
            [20, 42, 240, 28],
            json!(if stage == 0 { "" } else { "hello" }),
        ),
        ("accept", [20, 90, 180, 24], json!(stage >= 2)),
        (
            "amount",
            [220, 90, 240, 24],
            json!(if stage >= 3 { 50.0 } else { 25.0 }),
        ),
        ("submit", [280, 42, 100, 28], Value::Null),
    ] {
        let n = node(tree, key)?;
        equal(
            key,
            "bounds",
            json!(n.bounds),
            json!(bounds.map(|v| i64::from(v) * i64::from(scale))),
        )?;
        equal(key, "value", n.value.clone(), value)?;
        equal(
            key,
            "enabled",
            json!(n.flags.contains(&Flag::Enabled)),
            json!(true),
        )?;
        equal(
            key,
            "visible",
            json!(n.flags.contains(&Flag::Visible)),
            json!(true),
        )?;
        equal(
            key,
            "focused",
            json!(n.flags.contains(&Flag::Focused)),
            json!(key == focused),
        )?;
        equal(
            key,
            "pressed",
            json!(n.flags.contains(&Flag::Pressed)),
            json!(false),
        )?;
    }
    equal(
        "accept",
        "checked",
        json!(node(tree, "accept")?.flags.contains(&Flag::Checked)),
        json!(stage >= 2),
    )
}

fn bounds(tree: &Tree, key: &str, rect: [i64; 4], scale: u32) -> Result<(), String> {
    equal(
        key,
        "bounds",
        json!(node(tree, key)?.bounds),
        json!(rect.map(|v| v * i64::from(scale))),
    )
}
fn flag(tree: &Tree, key: &str, f: Flag, expected: bool) -> Result<(), String> {
    equal(
        key,
        &format!("{f:?}"),
        json!(node(tree, key)?.flags.contains(&f)),
        json!(expected),
    )
}
pub fn menu(tree: &Tree, stage: usize, scale: u32) -> Result<(), String> {
    if stage > 2 {
        return Err("unknown menu stage".into());
    }
    bounds(tree, "root", [0, 0, 640, 400], scale)?;
    bounds(
        tree,
        "menu",
        [0, 0, 640, if stage == 1 { 66 } else { 22 }],
        scale,
    )?;
    equal(
        "root",
        "value",
        tree.root.value.clone(),
        json!({"submissions":0,"menu_action":if stage==2 {7} else {0}}),
    )?;
    equal(
        "menu/0",
        "name",
        json!(node(tree, "menu/0")?.name),
        json!("Actions"),
    )?;
    flag(tree, "menu/0", Flag::Expanded, stage == 1)?;
    if stage == 1 {
        bounds(tree, "menu/0", [0, 22, 140, 44], scale)?;
    }
    for (key, name, value, y) in [("menu/0/0", "Choose", 7, 22), ("menu/0/1", "Reset", 8, 44)] {
        let n = node(tree, key)?;
        equal(
            key,
            "role",
            json!(format!("{:?}", n.role)),
            json!("Menuitem"),
        )?;
        equal(key, "name", json!(n.name), json!(name))?;
        equal(key, "value", n.value.clone(), json!(value))?;
        bounds(tree, key, [0, y, 140, 22], scale)?;
        equal(
            key,
            "clip",
            json!(n.clip),
            json!([0, y, 140, 22].map(|v| v * i64::from(scale))),
        )?;
        flag(tree, key, Flag::Visible, stage == 1)?;
        flag(tree, key, Flag::Enabled, true)?;
    }
    equal(
        "entry",
        "value",
        node(tree, "entry")?.value.clone(),
        json!(""),
    )?;
    flag(tree, "entry", Flag::Focused, true)
}
pub fn list(tree: &Tree, stage: usize, scale: u32) -> Result<(), String> {
    if stage > 2 {
        return Err("unknown list stage".into());
    }
    bounds(tree, "root", [0, 0, 640, 400], scale)?;
    bounds(tree, "list", [20, 150, 200, 100], scale)?;
    equal(
        "root",
        "value",
        tree.root.value.clone(),
        json!({"submissions":0,"menu_action":0}),
    )?;
    equal(
        "list",
        "value",
        node(tree, "list")?.value.clone(),
        if stage == 2 {
            json!("item-07")
        } else {
            Value::Null
        },
    )?;
    flag(tree, "list", Flag::Focused, stage == 2)?;
    let offset = if stage == 0 { 0 } else { 80 };
    for i in 0i64..20 {
        let key = format!("list/item-{i:02}");
        let n = node(tree, &key)?;
        let y = 150 + i * 20 - offset;
        let top = y.max(150);
        let height = (y + 20).min(250).saturating_sub(top).max(0);
        equal(&key, "name", json!(n.name), json!(format!("Item {i:02}")))?;
        equal(
            &key,
            "role",
            json!(format!("{:?}", n.role)),
            json!("Listitem"),
        )?;
        bounds(tree, &key, [20, y, 200, 20], scale)?;
        equal(
            &key,
            "clip",
            json!(n.clip),
            json!([20, top, 200, height].map(|v| v * i64::from(scale))),
        )?;
        flag(tree, &key, Flag::Visible, height > 0)?;
        flag(tree, &key, Flag::Selected, stage == 2 && i == 7)?;
        flag(tree, &key, Flag::Enabled, true)?;
    }
    Ok(())
}
