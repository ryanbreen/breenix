//! Pure pointer resolution; session code supplies the current window visibility.
use libbui::tree::{Flag, Node};

// Widen before forming half-open endpoints: tree rectangles carry signed i64s.
fn edges([x, y, w, h]: [i64; 4]) -> Option<[i128; 4]> {
    if w <= 0 || h <= 0 {
        return None;
    }
    Some([
        i128::from(x),
        i128::from(y),
        i128::from(x) + i128::from(w),
        i128::from(y) + i128::from(h),
    ])
}
fn intersection(a: [i128; 4], b: [i128; 4]) -> Option<[i128; 4]> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// Resolve window-local bounds and clip against exposed rectangles in screen pixels.
/// Choose the largest intersection, breaking equal-area ties topmost then leftmost.
/// The caller confirms this screen point with the current scene's window hit test.
pub fn click_target(
    node: &Node,
    origin: (i32, i32),
    visible_region: &[[i64; 4]],
) -> Result<(i32, i32), String> {
    let missing = || format!("input-target: {} has no visible area", node.key);
    if !node.flags.contains(&Flag::Visible) {
        return Err(missing());
    }
    let bounds = edges(node.bounds).ok_or_else(missing)?;
    let clip = edges(node.clip).ok_or_else(missing)?;
    let mut local = intersection(bounds, clip).ok_or_else(missing)?;
    for i in [0, 2] {
        local[i] += i128::from(origin.0);
    }
    for i in [1, 3] {
        local[i] += i128::from(origin.1);
    }
    let best = visible_region
        .iter()
        .filter_map(|&rect| intersection(local, edges(rect)?))
        .max_by_key(|r| ((r[2] - r[0]) * (r[3] - r[1]), -r[1], -r[0]));
    let r = best.ok_or_else(missing)?;
    let x = r[0] + (r[2] - r[0]) / 2;
    let y = r[1] + (r[3] - r[1]) / 2;
    let x = i32::try_from(x)
        .map_err(|_| format!("input-target: {} x outside pointer range", node.key))?;
    let y = i32::try_from(y)
        .map_err(|_| format!("input-target: {} y outside pointer range", node.key))?;
    Ok((x, y))
}

/// Home to (0,0), then reach the screen point with exact relative deltas.
/// bwm adds each delta without acceleration and clamps it to 0..dimension-1.
pub fn home_then_move(
    screen: (usize, usize),
    target: (i32, i32),
) -> Result<[(i32, i32); 2], String> {
    let width =
        i32::try_from(screen.0).map_err(|_| "input-target: screen width outside pointer range")?;
    let height =
        i32::try_from(screen.1).map_err(|_| "input-target: screen height outside pointer range")?;
    if width <= 0
        || height <= 0
        || target.0 < 0
        || target.1 < 0
        || target.0 >= width
        || target.1 >= height
    {
        return Err(format!(
            "input-target: point {target:?} outside screen {screen:?}"
        ));
    }
    Ok([(-width, -height), target])
}

/// A stable widget address, resolved afresh for each new operation.
#[derive(Clone, Debug)]
pub struct Target {
    pub window: u64,
    pub key: String,
}
impl Target {
    pub fn new(window: u64, key: &str) -> Self {
        Self {
            window,
            key: key.into(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Resolved {
    pub window: u64,
    pub point: (i32, i32),
    pub scene: u64,
    pub commit: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub serial: u64,
    pub window: u64,
}
#[derive(Debug)]
pub struct Click {
    pub point: (i32, i32),
    pub press: Delivery,
    pub release: Delivery,
}
#[derive(Debug)]
pub struct Drag {
    pub press: Delivery,
    pub motions: Vec<Delivery>,
    pub release: Delivery,
}

/// The live implementation waits for source outcomes and processed commit watermarks.
/// Host implementations exercise the same ordering without a compositor.
pub trait Backend {
    fn resolve(&mut self, target: &Target) -> Result<Resolved, String>;
    fn move_to(&mut self, point: (i32, i32)) -> Result<(), String>;
    fn confirm(&mut self, resolved: &Resolved) -> Result<(), String>;
    fn button(&mut self, pressed: bool, window: u64) -> Result<Delivery, String>;
    fn motion(&mut self, delta: (i32, i32), window: u64) -> Result<Delivery, String>;
    fn wheel(&mut self, dy: i32, window: u64) -> Result<Delivery, String>;
}
pub struct Pointer<B: Backend> {
    backend: B,
}
impl<B: Backend> Pointer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    fn position(&mut self, target: &Target) -> Result<Resolved, String> {
        let resolved = self.backend.resolve(target)?;
        self.backend.move_to(resolved.point)?;
        self.backend.confirm(&resolved)?;
        Ok(resolved)
    }
    pub fn click(&mut self, target: &Target) -> Result<Click, String> {
        let resolved = self.position(target)?;
        // A delivery timeout may follow an accepted press; release on that path too.
        let press = self.backend.button(true, target.window);
        let release = self.backend.button(false, target.window);
        Ok(Click {
            point: resolved.point,
            press: press?,
            release: release?,
        })
    }
    pub fn scroll(&mut self, target: &Target, dy: i32) -> Result<Delivery, String> {
        self.position(target)?;
        self.backend.wheel(dy, target.window)
    }
    /// Steps are relative device-pixel deltas, sent within the existing capture. The hook can coordinate application changes between processed steps.
    pub fn drag(
        &mut self,
        target: &Target,
        steps: &[(i32, i32)],
        mut after: impl FnMut(usize, &mut B) -> Result<(), String>,
    ) -> Result<Drag, String> {
        self.position(target)?;
        let result: Result<(Delivery, Vec<Delivery>), String> = (|| {
            let press = self.backend.button(true, target.window)?;
            after(0, &mut self.backend)?;
            let mut motions = Vec::with_capacity(steps.len());
            for (index, &delta) in steps.iter().enumerate() {
                motions.push(self.backend.motion(delta, target.window)?);
                after(index + 1, &mut self.backend)?;
            }
            Ok((press, motions))
        })();
        let release = self.backend.button(false, target.window);
        let (press, motions) = result?;
        Ok(Drag {
            press,
            motions,
            release: release?,
        })
    }
}

/// Subtract opaque, screen-space window frames from the target's exposed region.
pub fn exposed_region(content: [i64; 4], screen: [i64; 4], covers: &[[i64; 4]]) -> Vec<[i64; 4]> {
    let Some(r) = edges(content).and_then(|r| intersection(r, edges(screen)?)) else {
        return Vec::new();
    };
    let mut rects = vec![r];
    for &cover in covers {
        let Some(cover) = edges(cover) else { continue };
        rects = rects
            .into_iter()
            .flat_map(|r| {
                let Some(cut) = intersection(r, cover) else {
                    return vec![r];
                };
                [
                    [r[0], r[1], r[2], cut[1]],
                    [r[0], cut[3], r[2], r[3]],
                    [r[0], cut[1], cut[0], cut[3]],
                    [cut[2], cut[1], r[2], cut[3]],
                ]
                .into_iter()
                .filter(|p| p[0] < p[2] && p[1] < p[3])
                .collect()
            })
            .collect();
    }
    rects
        .into_iter()
        .map(|r| {
            [
                r[0] as i64,
                r[1] as i64,
                (r[2] - r[0]) as i64,
                (r[3] - r[1]) as i64,
            ]
        })
        .collect()
}
#[cfg(target_os = "breenix")]
#[path = "pointer_live.rs"]
mod live;
#[cfg(target_os = "breenix")]
pub use live::{wait_pointer_idle, LivePointer};
