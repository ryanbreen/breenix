//! Native checkpoint resource envelope and the parent sender's wire accounting.
use std::sync::Mutex;

pub(super) const LEGACY_TOTAL: usize = 64 << 20;
const SLOT: usize = 2 << 20;
const ENDS: &str = include_str!("../../../docs/suites/frames/checkpoints.json");
const INTENTS: &str = include_str!("../../../scripts/metal/intent-pictures.tsv");

pub(super) fn limit(suite: &str, modes: &[[u32; 3]], default: usize) -> Result<usize, String> {
    let mode = modes.get(default).ok_or("evidence-default-mode")?;
    if modes.is_empty() || modes.len() > 16 || modes.iter().any(|m|
        m[0] <= 64 || m[1] <= 64 || m[0] > 8192 || m[1] > 8192 || !matches!(m[2], 1 | 2)) {
        return Err("evidence-mode-list".into());
    }
    let section = ENDS.split_once(&format!("\"{suite}\": [")).ok_or("evidence-suite")?.1;
    let ends = section.lines().take_while(|s| !s.trim().starts_with(']'))
        .filter(|s| s.trim().starts_with("[\"")).count();
    let scale2 = modes.iter().filter(|m| m[2] == 2).max_by_key(|m| u64::from(m[0])*u64::from(m[1])).ok_or("evidence-scale2-mode")?;
    let smallest = modes.iter().min_by_key(|m| u64::from(m[0])*u64::from(m[1])).ok_or("evidence-mode-list")?;
    let target = if suite == "breengel" { scale2 } else { mode };
    let mut intents = 0;
    for row in INTENTS.lines().filter(|s| !s.starts_with('#')) {
        let f: Vec<_> = row.split('\t').collect();
        if f.get(1) != Some(&suite) { continue; }
        let mut source = [f[3].parse().map_err(|_| "evidence-inventory")?,
            f[4].parse().map_err(|_| "evidence-inventory")?];
        for rule in INTENTS.lines().filter_map(|line| line.strip_prefix("# size-mode ")) {
            let r: Vec<_> = rule.split_whitespace().collect();
            if r[0] == suite && r[1] == f[2] {
                let selected = match r[2] { "native" => mode, "scale2" => scale2,
                    "smallest" => smallest, _ => return Err("evidence-size-rule".into()) };
                source = [selected[0], selected[1]];
            }
        }
        intents += super::intent::picture_pages(suite, f[2], source, [target[0], target[1]])?.len();
    }
    let checkpoints = ends + intents + 4; // the four declared perturbation phases
    let pixels = modes.iter().map(|m| m[0] as usize * m[1] as usize * 4).max().ok_or("evidence-mode-list")?;
    // One worst-case sector-aligned SCR1 (pixels + 1024 name/header/padding)
    // and one 4096-byte CRC record per declared checkpoint, plus the two
    // 2 MiB startup/final slots and the retained 64 MiB native picture budget.
    checkpoints.checked_mul(pixels + 1024 + 4096)
        .and_then(|n| n.checked_add(2 * SLOT + LEGACY_TOTAL))
        .map(|n| n.max(LEGACY_TOTAL)).ok_or_else(|| "evidence-budget-overflow".into())
}

pub(super) struct Budget { limit: usize, bytes: Mutex<usize> }
impl Budget {
    pub fn new(limit: usize) -> Self { Self { limit, bytes: Mutex::new(0) } }
    pub fn limit(&self) -> usize { self.limit }
    pub fn check(&self, n: usize) -> Result<(), String> {
        let used = *self.bytes.lock().map_err(|_| "evidence-budget-poisoned")?;
        if n == 0 || used.checked_add(n).is_none_or(|n| n > self.limit) { Err("total-bound".into()) } else { Ok(()) }
    }
    pub fn reserve(&self, n: usize) -> Result<(), String> {
        let mut used = self.bytes.lock().map_err(|_| "evidence-budget-poisoned")?;
        *used = used.checked_add(n).filter(|&v| n > 0 && v <= self.limit).ok_or("total-bound")?;
        Ok(())
    }
}
