use raster::{Font, Layout};
use std::{cell::RefCell, sync::atomic::{AtomicBool, AtomicUsize, Ordering}};
#[cfg(target_arch="x86_64")]
static SHOWN_HEAP_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOWED: AtomicBool = AtomicBool::new(true);
fn may_draw() -> bool { ALLOWED.load(Ordering::Acquire) }
mod vt {
    use super::*;
    pub static FG: AtomicUsize = AtomicUsize::new(1);
    pub fn foreground() -> usize { FG.load(Ordering::Acquire) }
}
mod time {
    use super::*;
    pub static NOW: AtomicUsize = AtomicUsize::new(1000);
    pub fn get_monotonic_time() -> u64 { NOW.load(Ordering::Acquire) as u64 }
}
mod boot_ribbon {
    use super::*;
    pub fn drain() {}
    pub fn reached() -> Vec<bool> { vec![true, false] }
    pub fn suite_status() -> Option<status::SuiteStatus> {
        let mut suite = status::SuiteStatus::default();
        suite.observe(b"SUITE console START cases=88", 0);
        Some(suite)
    }
}
struct Display { pixels: RefCell<Vec<u8>>, layout: Layout }
impl Display {
    fn write(&self, shadow: &[u8], rows: core::ops::Range<usize>, allowed: fn() -> bool) -> bool {
        if !allowed() { return false; }
        let bytes = rows.start*self.layout.stride_bytes..rows.end*self.layout.stride_bytes;
        self.pixels.borrow_mut()[bytes.clone()].copy_from_slice(&shadow[bytes]);
        true
    }
    fn flush(&self, _: core::ops::Range<usize>) -> Result<(), &'static str> { Ok(()) }
}
#[derive(PartialEq)]
struct Shown { text: Vec<grid::Cell>, cursor: Option<usize> }
struct Snapshot { fg: usize, rows: usize, cols: usize, cursor: (usize, usize), cursor_visible: bool, cells: Vec<grid::Cell> }
impl Snapshot { fn row(&self, row: usize) -> &[grid::Cell] { &self.cells[row*self.cols..(row+1)*self.cols] } }
struct Screen {
    display: Display, layout: Layout, shadow: Vec<u8>, shown: Vec<Option<Shown>>,
    cleared: bool, complete: bool, frames: u64, last_failure: Option<&'static str>,
    panel: Option<panel::Panel>, panel_dirty: bool, panel_updated_ms: Option<u64>, ribbon_drop: bool,
}
impl Screen {
    REFRESH_METHOD
    UPDATE_METHOD
    fn pass(&mut self, snapshot: &Snapshot) { REFRESH_CALL self.update(snapshot); }
    fn invalidate(&mut self) {
        self.cleared=false; self.complete=false; self.shown.fill_with(|| None);
        self.panel_dirty=true; self.panel_updated_ms=None;
    }
}
#[test]
fn panel_stays_on_tty1_and_background_scanout_and_shadow_stay_identical() {
    assert_eq!(vt::foreground(),1);
    let layout=Layout { width:1280,height:800,bytes_per_pixel:4,stride_bytes:1280*4,
        reserved:255,bgr:true,font:Font::for_display(1280,800) };
    let mut screen=Screen { display:Display { pixels:RefCell::new(vec![0;layout.bytes()]),layout },layout,
        shadow:vec![0;layout.bytes()],shown:(0..layout.rows()).map(|_| None).collect(),
        cleared:false,complete:false,frames:0,last_failure:None,panel:None,panel_dirty:true,
        panel_updated_ms:None,ribbon_drop:false };
    let mut snapshot=Snapshot { fg:0,rows:layout.rows(),cols:layout.cols(),cursor:(0,0),
        cursor_visible:false,cells:vec![grid::BLANK;layout.rows()*layout.cols()] };
    screen.pass(&snapshot);
    let top=layout.panel().unwrap().panel.y*layout.stride_bytes;
    assert!(screen.shadow[top..].chunks_exact(4).any(|p| !layout.pixel_is(p,raster::PALETTE[0])));
    assert_eq!(screen.shadow,*screen.display.pixels.borrow());
    for fg in [1,5] {
        vt::FG.store(fg+1,Ordering::Release); snapshot.fg=fg; screen.invalidate();
        time::NOW.fetch_add(1000,Ordering::Release); screen.pass(&snapshot);
        let baseline=screen.display.pixels.borrow().clone();
        assert!(baseline[top..].chunks_exact(4).all(|p| layout.pixel_is(p,raster::PALETTE[0])),"tty1 panel painted on tty{}",fg+1);
        time::NOW.fetch_add(1000,Ordering::Release); screen.pass(&snapshot);
        assert_eq!(baseline,*screen.display.pixels.borrow(),"background panel changed the scanout");
        assert_eq!(baseline,screen.shadow,"switch digest shadow changed after publication");
    }
    vt::FG.store(1,Ordering::Release); snapshot.fg=0; screen.invalidate(); screen.pass(&snapshot);
    let baseline=screen.display.pixels.borrow().clone();
    ALLOWED.store(false,Ordering::Release); time::NOW.fetch_add(1000,Ordering::Release); screen.pass(&snapshot);
    assert_eq!(baseline,*screen.display.pixels.borrow(),"panel painted while display was owned");
    assert_eq!(baseline,screen.shadow,"unpublished panel changed the shadow while display was owned");
    ALLOWED.store(true,Ordering::Release); screen.invalidate(); screen.pass(&snapshot);
    assert!(screen.shadow[top..].chunks_exact(4).any(|p| !layout.pixel_is(p,raster::PALETTE[0])));
    assert_eq!(screen.shadow,*screen.display.pixels.borrow());
}
