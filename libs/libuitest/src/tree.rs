//! Pure queries over a committed semantic tree. Expected geometry comes from the scenario.
pub use libbui::tree::{Node, Role, Tree};

/// Present fields are matched together, exactly and case-sensitively.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query<'a> {
    pub key: Option<&'a str>,
    pub role: Option<Role>,
    pub name: Option<&'a str>,
}

fn nodes(tree: &Tree) -> impl Iterator<Item = &Node> {
    core::iter::once(&tree.root).chain(tree.children.iter())
}

/// Return a unique match; ambiguity includes the two matching keys and names.
pub fn find<'a>(tree: &'a Tree, query: &Query<'_>) -> Result<&'a Node, String> {
    let mut matches = nodes(tree).enumerate().filter(|(_, node)| {
        query.key.is_none_or(|key| node.key == key)
            && query.role.is_none_or(|role| node.role == role)
            && query.name.is_none_or(|name| node.name == name)
    });
    let Some((first_index, first)) = matches.next() else {
        return Err(format!("tree-query: no match for {query:?}"));
    };
    if let Some((second_index, second)) = matches.next() {
        return Err(format!(
            "tree-query: ambiguous {query:?}: node[{first_index}] key={:?} name={:?}, node[{second_index}] key={:?} name={:?}",
            first.key, first.name, second.key, second.name
        ));
    }
    Ok(first)
}

fn contains(outer: [i64; 4], inner: [i64; 4]) -> bool {
    outer[2] >= 0 && outer[3] >= 0 && inner[2] >= 0 && inner[3] >= 0
        && inner[0] >= outer[0] && inner[1] >= outer[1]
        && i128::from(inner[0]) + i128::from(inner[2]) <= i128::from(outer[0]) + i128::from(outer[2])
        && i128::from(inner[1]) + i128::from(inner[3]) <= i128::from(outer[1]) + i128::from(outer[3])
}

/// Check menu-item, tab and scrollbar-thumb parent roles and geometric containment.
/// This validates claims within the tree; it does not certify drawn geometry.
pub fn role_invariants(tree: &Tree) -> Result<(), String> {
    for node in nodes(tree) {
        let required = match node.role {
            Role::Menuitem => Role::Menu,
            Role::Tab => Role::Tablist,
            Role::ScrollThumb => Role::Scrollbar,
            _ => continue,
        };
        let fail = |detail: String| format!("tree-mismatch: role-invariants({}) {detail}", node.key);
        let key = node.parent.as_deref().ok_or_else(|| fail(format!("parent missing; expected {required:?}")))?;
        let parent = find(tree, &Query { key: Some(key), ..Query::default() })
            .map_err(|err| fail(format!("parent {key:?}: {err}")))?;
        if parent.role != required {
            return Err(fail(format!("parent {key:?} role expected={required:?} reported={:?}", parent.role)));
        }
        if !contains(parent.bounds, node.bounds) {
            return Err(fail(format!("bounds {:?} outside parent {key:?} bounds {:?}", node.bounds, parent.bounds)));
        }
    }
    for track in nodes(tree).filter(|node| node.role == Role::Scrollbar) {
        let count = nodes(tree).filter(|node| node.role == Role::ScrollThumb && node.parent.as_deref() == Some(track.key.as_str())).count();
        if count != 1 {
            return Err(format!("tree-mismatch: role-invariants({}) expected one thumb, reported={count}", track.key));
        }
    }
    Ok(())
}

/// Compare with a scenario-authored literal, independently of containment checks.
pub fn expect_bounds(node: &Node, expected: [i64; 4]) -> Result<(), String> {
    if node.bounds == expected { return Ok(()); }
    Err(crate::messages::tree(&format!("expect_bounds({})", node.key), &node.key, "bounds", &expected, &node.bounds))
}
