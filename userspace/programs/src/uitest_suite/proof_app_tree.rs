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
