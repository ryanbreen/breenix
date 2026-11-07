//! Production compositor with forced serialized snapshot replies and CPU BGL transport.
use std::{cell::RefCell, collections::BTreeMap};
#[path = "compositor.rs"]
mod compositor;
const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1600;
const AREA: usize = (WIDTH * HEIGHT) as usize;
// PATTERN
#[derive(Default)]
struct Device {
    next: u64,
    targets: BTreeMap<u64, Vec<u32>>,
    bound: u64,
    owner: bool,
    serial: u64,
    dirty: bool,
    per_window: Vec<u64>,
}
thread_local! { static DEVICE: RefCell<Device> = RefCell::new(Device::default()); }
fn device<T>(f: impl FnOnce(&mut Device) -> T) -> T { DEVICE.with(|d| f(&mut d.borrow_mut())) }
fn handle(words: &[u32], at: usize) -> u64 { words[at] as u64 | (words[at + 1] as u64) << 32 }
impl Device {
    // Only the BEGIN/SCISSOR/CLEAR/COPY emitted by this hidden-cursor case.
    fn execute(&mut self, words: &[u32], scene: u64) -> (u64, u64) {
        use gpu_abi::stream::cmd;
        let mut at = 0;
        let mut target = 0;
        let mut clip = [0, 0, WIDTH, HEIGHT];
        let mut written = vec![false; AREA];
        while at < words.len() {
            match words[at] {
                cmd::BEGIN => { target = handle(words, at + 2); clip = [0, 0, WIDTH, HEIGHT]; }
                cmd::SCISSOR => clip.copy_from_slice(&words[at + 2..at + 6]),
                cmd::CLEAR => {
                    for y in clip[1]..clip[3] { for x in clip[0]..clip[2] {
                        let i = (y * WIDTH + x) as usize;
                        self.targets.get_mut(&target).unwrap()[i] = words[at + 2];
                        if target == scene { written[i] = true; }
                    } }
                }
                cmd::COPY => {
                    let source = handle(words, at + 2);
                    let [sx, sy, w, h, dx, dy] = [4, 5, 6, 7, 10, 11].map(|k| words[at + k]);
                    assert_eq!(handle(words, at + 8), target);
                    let stride = if source == 1 { WINDOW_WIDTH } else { WIDTH };
                    let pixels = self.targets[&source].clone();
                    for y in 0..h { for x in 0..w {
                        let (tx, ty) = (dx + x, dy + y);
                        if tx < clip[0] || tx >= clip[2] || ty < clip[1] || ty >= clip[3] { continue; }
                        let i = (ty * WIDTH + tx) as usize;
                        self.targets.get_mut(&target).unwrap()[i] = pixels[((sy + y) * stride + sx + x) as usize];
                        if target == scene { written[i] = true; }
                    } }
                }
                op => panic!("unexpected BGL command {op}"),
            }
            at += words[at + 1] as usize;
        }
        let window = (0..WINDOW_HEIGHT).flat_map(|y| (0..WINDOW_WIDTH).map(move |x| (y * WIDTH + x) as usize));
        (written.iter().filter(|v| **v).count() as u64, window.filter(|&i| written[i]).count() as u64)
    }
}
mod gpu {
    pub use gpu_abi as abi;
    use abi::Plain;
    pub fn context_create() -> Result<u64, i64> { Ok(super::device(|d| { d.next += 1; d.next })) }
    pub fn context_destroy(_: u64) -> i64 { 0 }
    pub fn resource_destroy(id: u64) -> i64 {
        super::device(|d| { if d.bound != id { d.targets.remove(&id); } });
        0
    }
    pub fn modes() -> (i64, abi::Modes) {
        let mut modes = abi::Modes::ZERO;
        modes.modes[0] = abi::Mode { width: super::WIDTH, height: super::HEIGHT, scale: 2, reserved: 0 };
        (0, modes)
    }
    pub fn target_create(_: u32, w: u32, h: u32) -> Result<u64, i64> {
        assert_eq!((w, h), (super::WIDTH, super::HEIGHT));
        Ok(super::device(|d| { d.next += 1; let id = d.next; d.targets.insert(id, vec![0xA5A5A5A5; super::AREA]); id }))
    }
    pub fn scanout_target_create(f: u32, w: u32, h: u32) -> Result<u64, i64> { target_create(f, w, h) }
    pub fn submit(_: u64, words: &[u32]) -> (i64, abi::Submit) {
        super::device(|d| { d.execute(words, 0); });
        (0, abi::Submit::ZERO)
    }
    pub fn wait(_: u64, _: u64, _: u64) -> (i64, abi::Wait) { (0, abi::Wait::ZERO) }
}
mod time {
    pub struct Instant;
    impl Instant { pub fn as_nanos(&self) -> i128 { 1 } }
    pub fn now_monotonic() -> Result<Instant, i64> { Ok(Instant) }
    pub fn sleep_ms(_: u64) -> Result<(), i64> { Ok(()) }
}
mod window {
    pub use breengel_abi as abi;
    use abi::Plain;
    pub unsafe fn raw_call(op: u32, ptr: *mut u8, _: usize) -> i64 {
        super::device(|d| match op {
            abi::op::COMPOSITE_SNAPSHOT => {
                let s = unsafe { &mut *ptr.cast::<abi::SceneSnapshot>() };
                let reserve = s.window_capacity != 0;
                s.width = super::WIDTH; s.height = super::HEIGHT;
                s.window_count = 1; s.damage_count = u32::from(d.dirty);
                s.flags = u32::from(d.serial == 0);
                if reserve {
                    let w = abi::SceneWindow { id: 1, sample: 1, width: WINDOW_WIDTH, height: WINDOW_HEIGHT,
                        ..abi::SceneWindow::ZERO };
                    unsafe { (s.windows as *mut abi::SceneWindow).write(w); }
                    if d.dirty {
                        unsafe { (s.damage as *mut abi::Rect).write(abi::Rect::new(0, 0, super::WIDTH as i32, super::HEIGHT as i32)); }
                    }
                    s.snapshot = d.serial + 1;
                }
                0
            }
            abi::op::COMPOSITE => {
                let c = unsafe { &mut *ptr.cast::<abi::Composite>() };
                let words = unsafe { std::slice::from_raw_parts(c.stream as *const u32, c.length as usize / 4) };
                let (pixels, window) = d.execute(words, c.scene_target);
                d.bound = c.target; d.serial += 1; d.dirty = false;
                d.per_window.push(window);
                c.serial = d.serial; c.pixels = pixels;
                0
            }
            other => panic!("unexpected window op {other}"),
        })
    }
}
mod screen {
    type Snapshot = Result<(Vec<u32>, u64), String>;
    // END_CHECKPOINT
    pub(super) fn snapshot() -> Snapshot {
        super::device(|d| if d.owner { Ok((d.targets[&d.bound].clone(), d.bound)) } else { Err("no-case-scanout".into()) })
    }
    pub(super) fn finish(end: &mut EndCheckpoint) -> Snapshot {
        // END_SELECTION
    }
}
fn exercise(mine_first: bool, directory: &std::path::Path) {
    device(|d| {
        *d = Device { next: 1, owner: true, dirty: true, ..Device::default() };
        let mut source = Vec::new();
        for y in 0..WINDOW_HEIGHT { for x in 0..WINDOW_WIDTH {
            let mut p = pattern(WINDOW_SEED, x, y); p[3] = 255;
            source.push(u32::from_le_bytes(p));
        } }
        d.targets.insert(1, source);
    });
    let mut mine = compositor::Compositor::new().unwrap();
    let mut theirs = compositor::Compositor::new().unwrap();
    assert_ne!(mine.context(), theirs.context());
    assert!(mine.pending().unwrap());
    let a = if mine_first { mine.compose() } else { theirs.compose() }.unwrap();
    assert_eq!(a.serial, 1);
    assert_eq!(a.pixels, AREA as u64);
    device(|d| assert_eq!(d.targets[&d.bound][0], 0xFF1327F7, "first caller must draw seed 19"));
    // traced(theirs) drops its local compositor when that caller returns.
    let b = if mine_first {
        let frame = theirs.compose().unwrap();
        drop(theirs);
        frame
    } else {
        drop(theirs);
        mine.compose().unwrap()
    };
    assert_eq!((b.serial, b.pixels), (2, 0));
    assert!(!mine.pending().unwrap());
    device(|d| assert_eq!(d.per_window, [640 * 400, 0]));
    // The temporary caller has returned. Its binding hold preserves pixels even
    // after its user handles die; no other scene replaces the last binding.
    device(|d| assert!(d.targets[&d.bound].iter().all(|&p| p == 0xFF000000)));
    let mut end = screen::EndCheckpoint { result: None };
    end.before_drop(screen::snapshot);
    device(|d| d.owner = false); // Helper killed only after the pre-drop observation.
    drop(mine);
    let (pixels, binding) = screen::finish(&mut end).unwrap();
    assert_ne!(binding, 0);
    assert!(pixels.iter().all(|&p| p == 0xFF000000));
    let name = if mine_first { "mine-first" } else { "theirs-first" };
    let bytes: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
    std::fs::write(directory.join(format!("{name}.bgra")), bytes).unwrap();
    println!("{name}: first origin #1327F7, second/end entirely #000000; per-window pixels 256000,0");
}
fn main() {
    let path = std::env::args_os().nth(1).unwrap();
    for mine_first in [true, false] { exercise(mine_first, std::path::Path::new(&path)); }
}
