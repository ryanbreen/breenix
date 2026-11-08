//! Semantic tree page encoding shared by GUI producers and test tools.
use alloc::{borrow::ToOwned, format, string::{String, ToString}, vec::Vec};

use serde_json::{json, Value};

macro_rules! names {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl $name {
            fn wire(self) -> &'static str { match self { $(Self::$variant => $wire),+ } }
            fn parse(value: &str) -> Option<Self> { match value { $($wire => Some(Self::$variant),)+ _ => None } }
        }
    };
}
names!(Role {
    Window => "window", Button => "button", Checkbox => "checkbox", Slider => "slider",
    Textfield => "textfield", Label => "label", Text => "text", Tablist => "tablist",
    Tab => "tab", Menubar => "menubar", Menu => "menu", Menuitem => "menuitem",
    List => "list", Listitem => "listitem", Scrollbar => "scrollbar", Canvas => "canvas",
    Image => "image", Terminal => "terminal", Dialog => "dialog", Status => "status"
});
names!(Flag {
    Focused => "focused", Enabled => "enabled", Visible => "visible", Selected => "selected",
    Checked => "checked", Pressed => "pressed", Hovered => "hovered", Expanded => "expanded",
    Editable => "editable", Truncated => "truncated"
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub key: String,
    pub parent: Option<String>,
    pub role: Role,
    pub name: String,
    pub value: Value,
    pub bounds: [i64; 4],
    pub clip: [i64; 4],
    pub flags: Vec<Flag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree { pub root: Node, pub children: Vec<Node> }

impl Node {
    fn json(&self) -> Value {
        json!({"key": self.key, "parent": self.parent, "role": self.role.wire(),
            "name": self.name, "value": self.value, "bounds": self.bounds, "clip": self.clip,
            "flags": self.flags.iter().map(|flag| flag.wire()).collect::<Vec<_>>()})
    }
    fn parse(value: &Value) -> Result<Self, String> {
        let field = |name: &str| value.get(name).ok_or_else(|| format!("missing {name}"));
        let string = |name: &str| -> Result<String, String> {
            field(name)?.as_str().map(str::to_owned).ok_or_else(|| format!("invalid {name}"))
        };
        let rect = |name: &str| -> Result<[i64; 4], String> {
            let a = field(name)?.as_array().ok_or_else(|| format!("invalid {name}"))?;
            if a.len() != 4 { return Err(format!("invalid {name}")); }
            let mut out = [0; 4];
            for (slot, v) in out.iter_mut().zip(a) { *slot = v.as_i64().ok_or_else(|| format!("invalid {name}"))?; }
            Ok(out)
        };
        let parent = match field("parent")? { Value::Null => None, Value::String(s) => Some(s.clone()), _ => return Err("invalid parent".into()) };
        let role = Role::parse(&string("role")?).ok_or("invalid role")?;
        let flags = field("flags")?.as_array().ok_or("invalid flags")?.iter()
            .map(|v| v.as_str().and_then(Flag::parse).ok_or_else(|| "invalid flag".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { key: string("key")?, parent, role, name: string("name")?,
            value: field("value")?.clone(), bounds: rect("bounds")?, clip: rect("clip")?, flags })
    }
}
impl Tree {
    fn encode(root: &Node, children: &[Node]) -> Vec<u8> {
        let mut value = root.json();
        value["children"] = Value::Array(children.iter().map(Node::json).collect());
        let mut bytes = value.to_string().into_bytes();
        bytes.push(0);
        bytes
    }
    /// Write a complete JSON document and terminator, keeping only whole children.
    pub fn write(&self, page: &mut [u8]) -> Result<usize, String> {
        let full = Self::encode(&self.root, &self.children);
        if full.len() <= page.len() { page[..full.len()].copy_from_slice(&full); return Ok(full.len()); }
        let mut root = self.root.clone();
        if !root.flags.contains(&Flag::Truncated) { root.flags.push(Flag::Truncated); }
        let mut bytes = Self::encode(&root, &[]);
        if bytes.len() > page.len() { return Err("tree root does not fit".into()); }
        for count in 1..=self.children.len() {
            let candidate = Self::encode(&root, &self.children[..count]);
            if candidate.len() > page.len() { break; }
            bytes = candidate;
        }
        page[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
    /// Parse the first NUL-terminated JSON document in a tree page.
    pub fn parse(page: &[u8]) -> Result<Self, String> {
        let end = page.iter().position(|&b| b == 0).ok_or("unterminated tree")?;
        let value: Value = serde_json::from_slice(&page[..end]).map_err(|e| e.to_string())?;
        let root = Node::parse(&value)?;
        let children = match value.get("children") {
            None => Vec::new(),
            Some(Value::Array(nodes)) => nodes.iter().map(Node::parse).collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("invalid children".into()),
        };
        Ok(Self { root, children })
    }
}
