//! FrameChecks v1, shared by guest validation and tests/suite_manifests.rs.
//! Geometry uses disjoint rectangle sets for mask union accounting.
use serde_json::Map;
pub use serde_json::{from_str, Value};

#[derive(Clone, Copy, Debug)]
pub enum ParamType {
    U32(u32, u32),
    I32(i32, i32),
    Colour,
    Rect,
    WindowKey,
    Seed,
}
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamType,
}
#[derive(Clone, Copy, Debug)]
pub struct Formula {
    pub name: &'static str,
    pub params: &'static [Param],
}

pub struct Context<'a> {
    /// The frame extent is deferred during manifest loading and supplied at capture.
    pub frame: Option<(u32, u32)>,
    pub formulas: &'a [Formula],
    pub previous: Option<&'a Check>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}
impl Rect {
    fn right(self) -> u32 {
        self.x + self.w
    }
    fn bottom(self) -> u32 {
        self.y + self.h
    }
    fn area(self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }
    fn encloses(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }
    fn intersection(self, other: Self) -> Option<Self> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (r, b) = (
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        );
        (r > x && b > y).then(|| Self {
            x,
            y,
            w: r - x,
            h: b - y,
        })
    }
    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
}

/// A validated definition's comparison union and structural effective domain.
/// Keep this with its definition; revalidate with the actual frame extent before use.
#[derive(Debug)]
pub struct Check {
    pub case: String,
    pub check: String,
    pub source: Value,
    pub compared_area: u64,
    pub effective_area: u64,
    pub(crate) definition: Value,
    compared: Vec<Rect>,
    effective: Vec<Rect>,
}
impl Check {
    pub fn compares(&self, x: u32, y: u32) -> bool {
        self.compared.iter().any(|r| r.contains(x, y))
    }
    pub fn in_effective_domain(&self, x: u32, y: u32) -> bool {
        self.effective.iter().any(|r| r.contains(x, y))
    }
}

fn object<'a>(value: &'a Value, at: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{at}: expected object"))
}
fn fields(value: &Value, required: &[&str], optional: &[&str], at: &str) -> Result<(), String> {
    let obj = object(value, at)?;
    for key in obj.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("{at}: unknown field {key}"));
        }
    }
    for key in required {
        if !obj.contains_key(*key) {
            return Err(format!("{at}: missing {key}"));
        }
    }
    Ok(())
}
fn text<'a>(value: &'a Value, at: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{at}: expected non-empty text"))
}
fn uint(value: &Value, min: u64, max: u64, at: &str) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|n| *n >= min && *n <= max)
        .ok_or_else(|| format!("{at}: expected integer in [{min}, {max}]"))
}
fn array<'a>(value: &'a Value, at: &str) -> Result<&'a [Value], String> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{at}: expected array"))
}
fn colour(value: &Value, at: &str) -> Result<(), String> {
    let s = text(value, at)?;
    if s.len() != 7 || !s.starts_with('#') || !s.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err(format!("{at}: expected colour #rrggbb"));
    }
    Ok(())
}
fn rect(value: &Value, frame: Option<(u32, u32)>, named: bool, at: &str) -> Result<Rect, String> {
    let keys: &[&str] = if named {
        &["name", "x", "y", "w", "h"]
    } else {
        &["x", "y", "w", "h"]
    };
    fields(value, keys, &[], at)?;
    let mut n = [0u32; 4];
    for (i, k) in ["x", "y", "w", "h"].iter().enumerate() {
        n[i] = uint(
            &value[k],
            u64::from(i >= 2),
            u64::from(u32::MAX),
            &format!("{at}.{k}"),
        )? as u32;
    }
    let r = Rect {
        x: n[0],
        y: n[1],
        w: n[2],
        h: n[3],
    };
    if r.x.checked_add(r.w).is_none() || r.y.checked_add(r.h).is_none() {
        return Err(format!("{at}: rect endpoint overflow"));
    }
    if frame.is_some_and(|(w, h)| r.right() > w || r.bottom() > h) {
        return Err(format!("{at}: rect outside frame"));
    }
    Ok(r)
}

// Subtract one rectangle into disjoint strips, preserving the set partition.
fn subtract(set: Vec<Rect>, cut: Rect) -> Vec<Rect> {
    let mut out = Vec::new();
    for r in set {
        let Some(i) = r.intersection(cut) else {
            out.push(r);
            continue;
        };
        if r.y < i.y {
            out.push(Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: i.y - r.y,
            });
        }
        if i.bottom() < r.bottom() {
            out.push(Rect {
                x: r.x,
                y: i.bottom(),
                w: r.w,
                h: r.bottom() - i.bottom(),
            });
        }
        if r.x < i.x {
            out.push(Rect {
                x: r.x,
                y: i.y,
                w: i.x - r.x,
                h: i.h,
            });
        }
        if i.right() < r.right() {
            out.push(Rect {
                x: i.right(),
                y: i.y,
                w: r.right() - i.right(),
                h: i.h,
            });
        }
    }
    out
}
fn union_into(set: &mut Vec<Rect>, r: Rect) {
    let mut rest = vec![r];
    for &old in set.iter() {
        rest = subtract(rest, old);
    }
    set.extend(rest);
}
fn area(set: &[Rect]) -> u64 {
    set.iter().map(|r| r.area()).sum()
}
fn intersect(a: &[Rect], b: &[Rect]) -> Vec<Rect> {
    a.iter()
        .flat_map(|a| b.iter().filter_map(|b| a.intersection(*b)))
        .collect()
}
fn coverage(n: u64, total: u64) -> bool {
    n > 0 && u128::from(n) * 4 >= u128::from(total) * 3
}

fn formula_params(
    value: &Value,
    signature: &Formula,
    frame: Option<(u32, u32)>,
) -> Result<(), String> {
    let names: Vec<_> = signature.params.iter().map(|p| p.name).collect();
    fields(value, &names, &[], "params")?;
    for p in signature.params {
        let v = &value[p.name];
        let at = format!("params.{}", p.name);
        match p.kind {
            ParamType::U32(lo, hi) => {
                uint(v, u64::from(lo), u64::from(hi), &at)?;
            }
            ParamType::I32(lo, hi) => {
                if !v
                    .as_i64()
                    .is_some_and(|n| n >= i64::from(lo) && n <= i64::from(hi))
                {
                    return Err(format!("{at}: expected i32 in [{lo}, {hi}]"));
                }
            }
            ParamType::Colour => colour(v, &at)?,
            ParamType::Rect => {
                rect(v, frame, false, &at)?;
            }
            ParamType::WindowKey => {
                text(v, &at)?;
            }
            ParamType::Seed => {
                uint(v, 0, u64::MAX, &at)?;
            }
        }
    }
    Ok(())
}

/// Validate v1 fields and mask-dependent domains before comparison.
pub fn validate(value: &Value, ctx: &Context<'_>) -> Result<Check, String> {
    object(value, "frame check")?;
    uint(&value["version"], 1, 1, "version")?;
    let kind = text(&value["kind"], "kind")?;
    let mut required = vec![
        "version",
        "case",
        "check",
        "kind",
        "source",
        "at",
        "regions",
        "mask",
        "tolerance",
    ];
    match kind {
        "stored" => required.push("reference"),
        "formula" => required.extend(["formula", "params"]),
        "structural" => required.extend(["predicate", "params"]),
        _ => return Err(format!("kind: unknown {kind}")),
    }
    fields(value, &required, &[], "frame check")?;
    let case = text(&value["case"], "case")?.to_string();
    let check = text(&value["check"], "check")?.to_string();
    let source = &value["source"];
    match text(&source["kind"], "source.kind")? {
        "window" => {
            fields(source, &["kind", "window"], &[], "source")?;
            text(&source["window"], "source.window")?;
        }
        "screen" | "surface" => fields(source, &["kind"], &[], "source")?,
        _ => return Err("source: unknown kind".into()),
    }
    fields(&value["at"], &["checkpoint", "requires"], &[], "at")?;
    text(&value["at"]["checkpoint"], "at.checkpoint")?;
    for barrier in array(&value["at"]["requires"], "at.requires")? {
        text(barrier, "at.requires entry")?;
    }
    if kind == "stored" {
        let r = &value["reference"];
        fields(r, &["path", "sha256"], &[], "reference")?;
        text(&r["path"], "reference.path")?;
        let hash = text(&r["sha256"], "reference.sha256")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("reference.sha256: expected 64 hex digits".into());
        }
    }
    let mut regions = Vec::new();
    for value in array(&value["regions"], "regions")? {
        let r = rect(value, ctx.frame, true, "region")?;
        let name = text(&value["name"], "region.name")?;
        if regions.iter().any(|(old, _)| old == &name) {
            return Err("regions: names must be unique".into());
        }
        regions.push((name, r));
    }
    if regions.is_empty() {
        return Err("regions: expected non-empty list".into());
    }
    let mut masks = Vec::new();
    for m in array(&value["mask"], "mask")? {
        fields(m, &["rect", "reason"], &[], "mask")?;
        text(&m["reason"], "mask.reason")?;
        let r = rect(&m["rect"], ctx.frame, false, "mask.rect")?;
        if !regions.iter().any(|(_, region)| region.encloses(r)) {
            return Err("mask: rect outside declared regions".into());
        }
        masks.push(r);
    }
    let mut sets = Vec::new();
    let mut compared = Vec::new();
    for &(_, r) in &regions {
        let mut set = vec![r];
        for &m in &masks {
            if r.encloses(m) {
                set = subtract(set, m);
            }
        }
        if !coverage(area(&set), r.area()) {
            return Err("mask: must retain 75% of each region".into());
        }
        for &piece in &set {
            union_into(&mut compared, piece);
        }
        sets.push(set);
    }
    let compared_area = area(&compared);
    let tol = &value["tolerance"];
    fields(tol, &["channel", "maxDiffering"], &["reason"], "tolerance")?;
    let channel = uint(&tol["channel"], 0, 8, "tolerance.channel")?;
    let max = uint(
        &tol["maxDiffering"],
        0,
        compared_area / 1000,
        "tolerance.maxDiffering",
    )?;
    if channel != 0 || max != 0 || tol.get("reason").is_some() {
        text(&tol["reason"], "tolerance.reason")?;
    }
    let mut effective = compared.clone();
    if kind == "formula" {
        let name = text(&value["formula"], "formula")?;
        let sig = ctx
            .formulas
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| format!("formula: unknown signature {name}"))?;
        formula_params(&value["params"], sig, ctx.frame)?;
    }
    if kind == "structural" {
        let p = &value["params"];
        let predicate = text(&value["predicate"], "predicate")?;
        let keys: &[&str] = match predicate {
            "uniform" => &["region", "colour"],
            "outside" => &["region", "rect", "colour"],
            "count" => &["region", "min"],
            "equal-regions" => &["a", "b"],
            "differs-from-previous" => &["region", "minDiffering"],
            _ => return Err("predicate: unknown".into()),
        };
        fields(
            p,
            keys,
            if predicate == "count" { &["max"] } else { &[] },
            "params",
        )?;
        let operand = |key: &str| -> Result<usize, String> {
            let name = text(&p[key], &format!("params.{key}"))?;
            regions
                .iter()
                .position(|(n, _)| *n == name)
                .ok_or_else(|| format!("params.{key}: unknown region {name}"))
        };
        if predicate == "equal-regions" {
            let (a, b) = (operand("a")?, operand("b")?);
            let (ra, rb) = (regions[a].1, regions[b].1);
            if (ra.w, ra.h) != (rb.w, rb.h) {
                return Err("equal-regions: unequal sizes".into());
            }
            if ra.intersection(rb).is_some() {
                return Err("equal-regions: regions overlap".into());
            }
            let relative = |set: &[Rect], r: Rect| {
                set.iter()
                    .map(|s| Rect {
                        x: s.x - r.x,
                        y: s.y - r.y,
                        w: s.w,
                        h: s.h,
                    })
                    .collect::<Vec<_>>()
            };
            let pairs = intersect(&relative(&sets[a], ra), &relative(&sets[b], rb));
            if area(&pairs) < 2 || !coverage(area(&pairs), ra.area()) {
                return Err("equal-regions: insufficient effective domain pairs".into());
            }
            compared.clear();
            for pair in &pairs {
                for origin in [ra, rb] {
                    union_into(
                        &mut compared,
                        Rect {
                            x: pair.x + origin.x,
                            y: pair.y + origin.y,
                            ..*pair
                        },
                    );
                }
            }
            effective = pairs
                .into_iter()
                .map(|p| Rect {
                    x: p.x + ra.x,
                    y: p.y + ra.y,
                    ..p
                })
                .collect();
        } else {
            let i = operand("region")?;
            let r = regions[i].1;
            effective = sets[i].clone();
            match predicate {
                "uniform" => {
                    colour(&p["colour"], "params.colour")?;
                    if area(&effective) < 2 {
                        return Err("uniform: effective domain needs at least 2 pixels".into());
                    }
                }
                "outside" => {
                    colour(&p["colour"], "params.colour")?;
                    let inner = rect(&p["rect"], ctx.frame, false, "params.rect")?;
                    if !r.encloses(inner)
                        || inner.x <= r.x
                        || inner.y <= r.y
                        || inner.right() >= r.right()
                        || inner.bottom() >= r.bottom()
                    {
                        return Err("outside: rect must lie strictly inside region".into());
                    }
                    effective = subtract(effective, inner);
                    if !coverage(area(&effective), r.area() - inner.area()) {
                        return Err("outside: insufficient effective domain".into());
                    }
                }
                "count" => {
                    let min = uint(&p["min"], 2, u64::MAX, "params.min")?;
                    if let Some(max) = p.get("max") {
                        uint(max, min, u64::MAX, "params.max")?;
                    }
                    if min > area(&effective) {
                        return Err("count: min exceeds effective domain".into());
                    }
                }
                "differs-from-previous" => {
                    let min = uint(&p["minDiffering"], 1, u64::MAX, "params.minDiffering")?;
                    let prev = ctx
                        .previous
                        .filter(|p| p.source == *source && p.case == case)
                        .ok_or_else(|| {
                            format!(
                                "{}: missing previous check of same source in case {case}",
                                check
                            )
                        })?;
                    effective = intersect(&effective, &prev.compared);
                    if min > area(&effective) {
                        return Err("minDiffering exceeds previous/current effective domain".into());
                    }
                }
                _ => unreachable!(),
            }
            compared = effective.clone();
        }
    }
    let compared_area = area(&compared);
    let effective_area = area(&effective);
    if max > effective_area / 1000 {
        return Err("tolerance.maxDiffering exceeds effective domain cap".into());
    }
    Ok(Check {
        case,
        check,
        source: source.clone(),
        compared_area,
        effective_area,
        definition: value.clone(),
        compared,
        effective,
    })
}
