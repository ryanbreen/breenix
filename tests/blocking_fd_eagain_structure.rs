//! PR-A census: Pipe/FIFO repaired; Unix stream and Console/Tty inventoried only.
//! This deliberately bounded Rust recognizer fails closed on an unknown result
//! route. It is a structural regression check, not a Rust type checker.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Token(String),
    Group(char, Vec<Node>),
}
impl Node {
    pub fn token(&self) -> &str {
        if let Self::Token(t) = self {
            t
        } else {
            ""
        }
    }
    pub fn group(&self, delimiter: char) -> Option<&[Node]> {
        match self {
            Self::Group(d, n) if *d == delimiter => Some(n),
            _ => None,
        }
    }
}
/// Erase comments and literal contents before balancing groups. Escapes, nested
/// block comments and raw strings cannot inject fake wake/error tokens.
pub fn lex(source: &str) -> Vec<Node> {
    fn scan(bytes: &[u8], at: &mut usize, end: u8) -> Vec<Node> {
        let mut out = Vec::new();
        while *at < bytes.len() {
            let c = bytes[*at];
            if c == end && end != 0 {
                *at += 1;
                return out;
            }
            if c.is_ascii_whitespace() {
                *at += 1;
                continue;
            }
            if bytes[*at..].starts_with(b"//") {
                while *at < bytes.len() && bytes[*at] != b'\n' {
                    *at += 1;
                }
                continue;
            }
            if bytes[*at..].starts_with(b"/*") {
                *at += 2;
                let mut depth = 1;
                while depth > 0 {
                    assert!(*at < bytes.len(), "unterminated comment");
                    if bytes[*at..].starts_with(b"/*") {
                        depth += 1;
                        *at += 2;
                    } else if bytes[*at..].starts_with(b"*/") {
                        depth -= 1;
                        *at += 2;
                    } else {
                        *at += 1;
                    }
                }
                continue;
            }
            let raw_start = if c == b'r' || (c == b'b' && bytes.get(*at + 1) == Some(&b'r')) {
                let mut p = *at + if c == b'b' { 2 } else { 1 };
                while bytes.get(p) == Some(&b'#') {
                    p += 1;
                }
                (bytes.get(p) == Some(&b'"')).then_some(p)
            } else {
                None
            };
            if let Some(quote) = raw_start {
                let hashes = quote - *at - if c == b'b' { 2 } else { 1 };
                *at = quote + 1;
                loop {
                    assert!(*at < bytes.len(), "unterminated raw string");
                    if bytes[*at] == b'"'
                        && bytes
                            .get(*at + 1..*at + 1 + hashes)
                            .is_some_and(|s| s.iter().all(|b| *b == b'#'))
                    {
                        *at += 1 + hashes;
                        break;
                    }
                    *at += 1;
                }
                out.push(Node::Token("LITERAL".into()));
                continue;
            }
            if c == b'"'
                || (c == b'\''
                    && (bytes.get(*at + 2) == Some(&b'\'') || bytes.get(*at + 1) == Some(&b'\\')))
            {
                *at += 1;
                loop {
                    assert!(*at < bytes.len(), "unterminated literal");
                    let q = bytes[*at];
                    *at += 1;
                    if q == b'\\' {
                        *at += 1;
                    } else if q == c {
                        break;
                    }
                }
                out.push(Node::Token("LITERAL".into()));
                continue;
            }
            if let Some(close) = match c {
                b'(' => Some(b')'),
                b'[' => Some(b']'),
                b'{' => Some(b'}'),
                _ => None,
            } {
                *at += 1;
                out.push(Node::Group(c as char, scan(bytes, at, close)));
                continue;
            }
            assert!(!b")] }".contains(&c) || c == b' ', "unbalanced group");
            let start = *at;
            if c.is_ascii_alphanumeric() || c == b'_' {
                *at += 1;
                while *at < bytes.len()
                    && (bytes[*at].is_ascii_alphanumeric() || bytes[*at] == b'_')
                {
                    *at += 1;
                }
            } else {
                *at += 1;
                if *at < bytes.len()
                    && [
                        b"::", b"=>", b"->", b"!=", b"==", b"<=", b">=", b"&&", b"||", b"+=",
                    ]
                    .iter()
                    .any(|p| &bytes[start..=*at] == *p)
                {
                    *at += 1;
                }
            }
            out.push(Node::Token(
                String::from_utf8(bytes[start..*at].to_vec()).unwrap(),
            ));
        }
        assert_eq!(end, 0, "unclosed group");
        out
    }
    scan(source.as_bytes(), &mut 0, 0)
}
pub fn compact(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|n| match n {
            Node::Token(t) => t.clone(),
            Node::Group(c, body) => format!(
                "{}{}{}",
                c,
                compact(body),
                match c {
                    '(' => ')',
                    '[' => ']',
                    _ => '}',
                }
            ),
        })
        .collect()
}
pub fn contains(nodes: &[Node], needle: &str) -> bool {
    compact(nodes).contains(needle)
}
pub fn read(path: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}
#[derive(Clone, Debug)]
pub struct Function {
    pub params: Vec<String>,
    pub bool_params: Vec<String>,
    pub body: Vec<Node>,
}
pub fn functions(nodes: &[Node]) -> BTreeMap<String, Function> {
    let mut out = BTreeMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.token() == "fn" {
            let name = nodes[i + 1].token().to_owned();
            let args_at = (i + 2..nodes.len())
                .find(|j| nodes[*j].group('(').is_some())
                .unwrap();
            let params = nodes[args_at]
                .group('(')
                .unwrap()
                .split(|n| n.token() == ",")
                .filter_map(|p| {
                    p.iter()
                        .position(|n| n.token() == ":")
                        .map(|j| p[j - 1].token().to_owned())
                })
                .collect();
            let bool_params = nodes[args_at]
                .group('(')
                .unwrap()
                .split(|n| n.token() == ",")
                .filter(|p| contains(p, ":bool"))
                .filter_map(|p| {
                    p.iter()
                        .position(|n| n.token() == ":")
                        .map(|j| p[j - 1].token().to_owned())
                })
                .collect();
            if let Some(body) = nodes[args_at + 1..]
                .iter()
                .take_while(|n| n.token() != ";")
                .find_map(|n| n.group('{'))
            {
                out.insert(
                    name,
                    Function {
                        params,
                        bool_params,
                        body: body.to_vec(),
                    },
                );
            }
        }
        if let Node::Group(_, inner) = n {
            out.extend(functions(inner));
        }
    }
    out
}
pub fn function(source: &str, name: &str) -> Vec<Node> {
    functions(&lex(source))
        .remove(name)
        .unwrap_or_else(|| panic!("missing function {name}"))
        .body
}

/// Match arms are top-level token spans terminated by commas or a block body.
/// Pattern groups are already balanced, so struct patterns cannot swallow bodies.
pub fn match_arms(nodes: &[Node], enum_name: &str) -> Vec<(Vec<String>, Vec<Node>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        if nodes[i].token() == enum_name && nodes.get(i + 1).is_some_and(|n| n.token() == "::") {
            if let Some(arrow) = (i..nodes.len())
                .take_while(|j| nodes[*j].token() != ",")
                .find(|j| nodes[*j].token() == "=>")
            {
                let names = nodes[i..arrow]
                    .windows(3)
                    .filter(|w| w[0].token() == enum_name && w[1].token() == "::")
                    .map(|w| w[2].token().to_owned())
                    .collect();
                let end = if nodes.get(arrow + 1).and_then(|n| n.group('{')).is_some() {
                    arrow + 2
                } else {
                    (arrow + 1..nodes.len())
                        .find(|j| nodes[*j].token() == ",")
                        .unwrap_or(nodes.len())
                };
                out.push((names, nodes[arrow + 1..end].to_vec()));
                i = end;
                continue;
            }
        }
        if let Node::Group(_, body) = &nodes[i] {
            out.extend(match_arms(body, enum_name));
        }
        i += 1;
    }
    out
}

fn boolean(nodes: &[Node], bindings: &BTreeMap<String, bool>) -> Option<bool> {
    if nodes.len() == 1 {
        if let Some(inner) = nodes[0].group('(') {
            return boolean(inner, bindings);
        }
        return match nodes[0].token() {
            "true" => Some(true),
            "false" => Some(false),
            t => bindings.get(t).copied(),
        };
    }
    if nodes.first().is_some_and(|n| n.token() == "!") {
        return boolean(&nodes[1..], bindings).map(|b| !b);
    }
    if let Some(i) = nodes.iter().position(|n| n.token() == "&&") {
        let a = boolean(&nodes[..i], bindings);
        let b = boolean(&nodes[i + 1..], bindings);
        return if a == Some(false) || b == Some(false) {
            Some(false)
        } else if a == Some(true) && b == Some(true) {
            Some(true)
        } else {
            None
        };
    }
    if contains(nodes, "status_flags") && contains(nodes, "&") && contains(nodes, "O_NONBLOCK") {
        if contains(nodes, "!=0") {
            return Some(false);
        }
        if contains(nodes, "==0") {
            return Some(true);
        }
    }
    None
}
/// Explore both unknown branches; prune only a derived mode predicate. Follow
/// local helpers and bind positional boolean arguments to renamed parameters.
fn inspect(
    nodes: &[Node],
    mut bindings: BTreeMap<String, bool>,
    defs: &BTreeMap<String, Function>,
    stack: &mut BTreeSet<String>,
) -> Result<(), String> {
    let mut i = 0;
    while i < nodes.len() {
        let token = nodes[i].token();
        if token == "let" {
            if let Some(semi) = (i + 1..nodes.len()).find(|j| nodes[*j].token() == ";") {
                if let Some(eq) = (i + 1..semi).find(|j| nodes[*j].token() == "=") {
                    if let Some(name) = nodes[i + 1..eq]
                        .iter()
                        .find(|n| !["mut", ""].contains(&n.token()))
                    {
                        if let Some(value) = boolean(&nodes[eq + 1..semi], &bindings) {
                            bindings.insert(name.token().to_owned(), value);
                        }
                    }
                }
            }
        }
        if token == "if" {
            if let Some(open) = (i + 1..nodes.len()).find(|j| nodes[*j].group('{').is_some()) {
                let test = boolean(&nodes[i + 1..open], &bindings);
                if test != Some(false) {
                    inspect(
                        nodes[open].group('{').unwrap(),
                        bindings.clone(),
                        defs,
                        stack,
                    )?;
                }
                i = open + 1;
                if nodes.get(i).is_some_and(|n| n.token() == "else") {
                    i += 1;
                    if let Some(other) = nodes.get(i).and_then(|n| n.group('{')) {
                        if test != Some(true) {
                            inspect(other, bindings.clone(), defs, stack)?;
                        }
                        i += 1;
                    } else {
                        return Err("unrecognized else route".into());
                    }
                }
                continue;
            }
        }
        if token == "EAGAIN" || token == "11" {
            return Err(format!("blocking route reaches {token}"));
        }
        if let Some(args) = nodes.get(i + 1).and_then(|n| n.group('(')) {
            if let Some(def) = defs.get(token) {
                if !stack.insert(token.to_owned()) {
                    return Err(format!("recursive/unrecognized route {token}"));
                }
                let mut call_bindings = BTreeMap::new();
                for (param, arg) in def.params.iter().zip(args.split(|n| n.token() == ",")) {
                    if let Some(value) = boolean(arg, &bindings) {
                        call_bindings.insert(param.clone(), value);
                    }
                }
                inspect(&def.body, call_bindings, defs, stack)?;
                stack.remove(token);
            } else if i == 0 || nodes[i - 1].token() != "." {
                // External primitive calls have explicit ownership elsewhere.
                // Unknown free functions (including extracted errno helpers)
                // cannot silently become an unexamined result route.
                const LEAVES: &[&str] = &[
                    "Ok",
                    "Err",
                    "Some",
                    "None",
                    "drop",
                    "clone",
                    "current_thread_id",
                    "check_signals_for_eintr",
                    "with_scheduler",
                    "yield_current",
                    "arch_halt_with_interrupts",
                    "preempt_enable",
                    "preempt_disable",
                    "from",
                    "min",
                ];
                if !LEAVES.contains(&token)
                    && !["let", "=", ",", "&", "|", "||", "return"].contains(&token)
                    && token
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                {
                    return Err(format!("unrecognized called route {token}"));
                }
            }
        }
        if let Node::Group(_, inner) = &nodes[i] {
            inspect(inner, bindings.clone(), defs, stack)?;
        }
        i += 1;
    }
    Ok(())
}
fn audit_helper(source: &str, entry: &str) -> Result<(), String> {
    let defs = functions(&lex(source));
    let f = defs
        .get(entry)
        .ok_or_else(|| format!("missing route {entry}"))?;
    let bindings = f.bool_params.iter().map(|p| (p.clone(), false)).collect();
    inspect(&f.body, bindings, &defs, &mut BTreeSet::new())
}

#[test]
fn parser_tracks_renames_negation_formatting_and_extracted_errors() {
    let good = "fn renamed(nonblocking: bool) { let should_park = !nonblocking; if !should_park { return error_leaf(); } Ok(0) } fn error_leaf() { Err(EAGAIN) }";
    assert!(audit_helper(good, "renamed").is_ok());
    let extracted = "fn route(nonblocking: bool) { renamed( nonblocking ) } fn renamed(mode: bool) { if mode { Err(EAGAIN) } else { Ok(0) } }";
    assert!(audit_helper(extracted, "route").is_ok());
    assert!(audit_helper(&extracted.replace("if mode", "if !mode"), "route").is_err());
    assert!(audit_helper(
        "fn route(nonblocking: bool) { opaque_error_helper() }",
        "route"
    )
    .is_err());
    assert!(audit_helper(
        "fn route(nonblocking: bool) { hidden() } fn hidden() { Err(11) }",
        "route"
    )
    .is_err());
    assert!(audit_helper(
        "fn route(nonblocking: bool) { /* Err(EAGAIN) */ let text = \"Err(11)\"; Ok(0) }",
        "route"
    )
    .is_ok());
}

#[test]
fn repaired_families_have_no_blocking_eagain_exit() {
    let handlers = read("kernel/src/syscall/handlers.rs");
    let nodes = lex(&handlers);
    let compacted = compact(&nodes);
    let mut discovered = BTreeSet::new();
    for pair in nodes.windows(3) {
        if pair[0].token() == "FdKind" && pair[1].token() == "::" {
            discovered.insert(pair[2].token().to_owned());
        }
    }
    // FdKind lives inside function groups: walk the derived token tree.
    fn census(nodes: &[Node], set: &mut BTreeSet<String>) {
        for w in nodes.windows(3) {
            if w[0].token() == "FdKind" && w[1].token() == "::" {
                set.insert(w[2].token().to_owned());
            }
        }
        for node in nodes {
            if let Node::Group(_, inner) = node {
                census(inner, set);
            }
        }
    }
    census(&nodes, &mut discovered);
    for family in ["PipeWrite", "FifoWrite", "UnixStream", "Device"] {
        assert!(discovered.contains(family), "census lost {family}");
    }
    let helper = read("kernel/src/syscall/blocking_io.rs");
    let defs = functions(&lex(&helper));
    let roots: Vec<_> = defs
        .keys()
        .filter(|name| compacted.contains(&format!("blocking_io::{name}(")))
        .collect();
    assert_eq!(
        roots.len(),
        1,
        "both adapters must reach one recognized shared writer"
    );
    assert_eq!(
        compacted
            .matches(&format!("blocking_io::{}(", roots[0]))
            .count(),
        1,
        "shared Pipe/Fifo match must delegate together"
    );
    let routes = match_arms(&nodes, "WriteOperation");
    for family in ["Pipe", "Fifo"] {
        let arms: Vec<_> = routes
            .iter()
            .filter(|(names, _)| names.iter().any(|name| name == family))
            .collect();
        assert_eq!(arms.len(), 1, "missing/ambiguous {family} result route");
        let snapshot_family = if family == "Pipe" {
            "PipeWrite"
        } else {
            "FifoWrite"
        };
        let snapshots = match_arms(&nodes, "FdKind");
        let snapshot = snapshots
            .iter()
            .find(|(names, _)| names.iter().any(|n| n == snapshot_family))
            .expect("snapshot arm discovered");
        fn mode_field(nodes: &[Node]) -> Option<String> {
            for node in nodes {
                if let Node::Group(_, body) = node {
                    for field in body.split(|n| n.token() == ",") {
                        if field.iter().any(|n| n.token() == ":")
                            && contains(field, "status_flags")
                            && contains(field, "&")
                            && contains(field, "O_NONBLOCK")
                            && contains(field, "!=0")
                        {
                            return Some(field[0].token().to_owned());
                        }
                    }
                    if let Some(found) = mode_field(body) {
                        return Some(found);
                    }
                }
            }
            None
        }
        let field = mode_field(&snapshot.1)
            .expect("mode field derived from status_flags & O_NONBLOCK != 0");
        fn pattern_binding(nodes: &[Node], family: &str, field: &str) -> Option<String> {
            for (i, node) in nodes.iter().enumerate() {
                if node.token() == "WriteOperation"
                    && nodes.get(i + 2).is_some_and(|n| n.token() == family)
                {
                    if let Some(pattern) = nodes.get(i + 3).and_then(|n| n.group('{')) {
                        for item in pattern.split(|n| n.token() == ",") {
                            if item.first().is_some_and(|n| n.token() == field) {
                                if item.len() == 1 {
                                    return Some(field.into());
                                }
                                if item.len() == 3
                                    && item[1].token() == ":"
                                    && !item[2].token().is_empty()
                                {
                                    return Some(item[2].token().into());
                                }
                            }
                        }
                    }
                }
                if let Node::Group(_, body) = node {
                    if let Some(binding) = pattern_binding(body, family, field) {
                        return Some(binding);
                    }
                }
            }
            None
        }
        let binding = pattern_binding(&nodes, family, &field)
            .expect("snapshotted mode destructured into adapter");
        let mut mode = BTreeMap::new();
        mode.insert(binding.clone(), false);
        inspect(&arms[0].1, mode, &defs, &mut BTreeSet::new())
            .unwrap_or_else(|e| panic!("{family} adapter: {e}"));
        assert!(
            contains(&arms[0].1, &format!(",{binding})")),
            "{family} lost owned handle or mode forwarding"
        );
    }
    audit_helper(&helper, roots[0])
        .unwrap_or_else(|e| panic!("Pipe/FIFO blocking route rejected: {e}"));
    // The unrepaired inventory remains explicit and does not assert a repair.
    assert!(
        contains(
            &function(&read("kernel/src/socket/unix.rs"), "write"),
            "EAGAIN"
        ) || contains(&function(&read("kernel/src/socket/unix.rs"), "write"), "11")
    );
    assert!(
        contains(
            &function(&read("kernel/src/fs/devfs/mod.rs"), "device_read"),
            "EAGAIN"
        ) || contains(
            &function(&read("kernel/src/fs/devfs/mod.rs"), "device_read"),
            "11"
        )
    );
    println!(
        "Pipe/FIFO: 0 prohibited blocking EAGAIN exits; Unix/Console: inventoried, unrepaired"
    );
}

#[test]
fn blocking_eagain_mutations_fail_the_live_helper_census() {
    let source = read("kernel/src/syscall/blocking_io.rs");
    audit_helper(&source, "write_pipe").unwrap();
    for terminal in [
        "return SyscallResult::Err(errno::EAGAIN as u64);",
        "return SyscallResult::Err(11);",
        "return hidden_error();",
    ] {
        let mutated = source.replace(
            "if is_nonblocking {",
            &format!("if !is_nonblocking {{ {terminal} }} if is_nonblocking {{"),
        );
        assert_ne!(mutated, source, "blocking mutation did not apply");
        assert!(
            audit_helper(&mutated, "write_pipe").is_err(),
            "{terminal} escaped census"
        );
    }
    let extracted = source.replace(
        "if is_nonblocking {",
        "if !is_nonblocking { return extracted_error(); } if is_nonblocking {",
    )
        + " fn extracted_error() -> SyscallResult { SyscallResult::Err(errno::EAGAIN as u64) }";
    assert!(audit_helper(&extracted, "write_pipe").is_err());
    let renamed = source
        .replace("write_pipe", "renamed_adapter")
        .replace("wait_prepared", "renamed_wait")
        .replace("progress_or_error", "renamed_result")
        .replace("is_nonblocking", "mode");
    audit_helper(&renamed, "renamed_adapter").expect("helper and mode rename remain recognized");
    let new_code = lex(&source);
    fn raw_errno(nodes: &[Node]) -> bool {
        nodes.iter().any(|n| match n {
            Node::Token(t) => t == "11" || t == "32",
            Node::Group(_, body) => raw_errno(body),
        })
    }
    assert!(
        !raw_errno(&new_code),
        "new syscall adapter must use named errnos in every mode"
    );
}
