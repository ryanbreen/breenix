//! Reader bookkeeping must preserve B12's raw CREATE charge.
use std::{fs, process::Command};

fn function<'a>(source: &'a str, needle: &str) -> &'a str {
    let start = source.find(needle).unwrap();
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    let end = source[brace..].bytes().position(|b| {
        if b == b'{' { depth += 1; }
        if b == b'}' { depth -= 1; }
        depth == 0
    }).unwrap();
    &source[start..=brace + end]
}

#[test]
fn raw_create_and_status_projection_have_separate_address_charges() {
    let registry = include_str!("../kernel/src/window/mod.rs");
    let accounting = function(registry, "pub fn charged_bytes(");
    let limits = include_str!("../kernel/src/process/limits.rs");
    let mapped = function(limits, "pub fn mapped_bytes(");
    let ops = include_str!("../kernel/src/window/ops.rs");
    let create = ops.split("let tree_len = count as u64").nth(1).unwrap();
    let start = create.find("room_for(").unwrap() + "room_for(".len();
    let mut depth = 1;
    let end = create[start..].bytes().position(|b| {
        if b == b'(' { depth += 1; }
        if b == b')' { depth -= 1; }
        depth == 0
    }).unwrap();
    let reservation = create[start..start + end].replace("mem::PAGE_BYTES", "window::mem::PAGE_BYTES");
    let fixture = format!(r#"
use std::sync::atomic::{{AtomicU64, Ordering}};
pub mod memory {{ pub mod vma {{
    pub struct Vma(pub u64);
    impl Vma {{ pub fn size(&self) -> u64 {{ self.0 }} }}
}} }}
pub mod gpu {{ pub mod window_targets {{
    pub fn backing_len(w: u32, h: u32) -> u64 {{ u64::from(w)*u64::from(h)*4 }}
}} }}
pub mod window {{
    use std::collections::BTreeMap;
    use std::sync::{{LazyLock, Mutex, MutexGuard}};
    pub mod mem {{
        pub const PAGE_BYTES: u64 = 4096;
        pub struct Mem(pub u64);
        pub fn unmapped_len(mem: &Mem, _: u64, vmas: &[crate::memory::vma::Vma]) -> u64 {{
            mem.0.saturating_sub(vmas.iter().map(|v| v.size()).sum())
        }}
        pub fn unmapped_status_len(mem: &Mem, _: &[crate::memory::vma::Vma]) -> u64 {{ mem.0 }}
    }}
    struct Window {{ owner: u64, tree: mem::Mem, tree_addr: u64, tree_readers: mem::Mem }}
    struct Remnant {{ holder: u64, charge: u64 }}
    struct Registry {{ windows: BTreeMap<u64, Window>, remnants: BTreeMap<u64, Remnant> }}
    struct Lock(Mutex<Registry>);
    impl Lock {{ fn lock(&self) -> MutexGuard<'_, Registry> {{ self.0.lock().unwrap() }} }}
    static REGISTRY: LazyLock<Lock> = LazyLock::new(|| Lock(Mutex::new(Registry {{
        windows: BTreeMap::new(), remnants: BTreeMap::new()
    }})));
    pub fn create() {{
        let mut reg = REGISTRY.lock();
        reg.windows.insert(1, Window {{ owner: 7, tree: mem::Mem(196608), tree_addr: 4096,
            tree_readers: mem::Mem(4096) }});
        assert_eq!(reg.windows[&1].tree_readers.0, 4096);
    }}
    pub fn destroy() {{ REGISTRY.lock().windows.clear(); }}
    {accounting}
}}
struct Id(u64);
impl Id {{ fn as_u64(&self) -> u64 {{ self.0 }} }}
struct Limits {{ gpu_bytes: AtomicU64 }}
struct Process {{ image_size: u64, heap_end: u64, heap_start: u64, user_stack_top: u64,
    user_stack_bottom: u64, vmas: Vec<memory::vma::Vma>, thread_group_id: Option<u64>, id: Id, limits: Limits }}
impl Process {{ {mapped} }}
fn main() {{
    window::create();
    let mut p = Process {{ image_size: 0, heap_end: 0, heap_start: 0, user_stack_top: 0,
        user_stack_bottom: 0, vmas: vec![memory::vma::Vma(196608)], thread_group_id: Some(7),
        id: Id(7), limits: Limits {{ gpu_bytes: AtomicU64::new(98304) }} }};
    assert_eq!(p.mapped_bytes(), 294912, "B12 raw CREATE includes trees and GPU backing, not kernel bookkeeping");
    p.vmas.push(memory::vma::Vma(4096));
    assert_eq!(p.mapped_bytes(), 299008, "the requested read-only status projection is charged as a VMA");
    p.vmas.pop();
    assert_eq!(p.mapped_bytes(), 294912);
    window::destroy(); p.vmas.clear(); p.limits.gpu_bytes.store(0, Ordering::Release);
    assert_eq!(p.mapped_bytes(), 0);
    struct Args {{ width: u32, height: u32 }}
    let a = Args {{ width: 128, height: 64 }}; let count = 3; let tree_len = 196608;
    assert_eq!({reservation}, 294912, "CREATE preflight reserves the B12 backing charge");
}}
"#);
    let dir = std::env::temp_dir().join(format!("window-reader-charge-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("charge.rs"); let binary = dir.join("charge");
    fs::write(&path, fixture).unwrap();
    let built = Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(&path).arg("-o").arg(&binary).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(&binary).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}
