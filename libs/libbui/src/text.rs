use libgfx::{bitmap_font, color::Color, surface::Surface, ttf_font::{self, CachedFont, Font}};
use crate::{rect::Rect, theme::Theme};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// The pinned font is immutable input data; only its outlines/crossings reach the GPU.
const SANS: &[u8] = include_bytes!("../../../fonts/DejaVuSans.ttf");
struct Slot { owner: AtomicU64, font: UnsafeCell<Option<CachedFont>> }
unsafe impl Sync for Slot {}
static FONT: Slot = Slot { owner: AtomicU64::new(0), font: UnsafeCell::new(None) };
fn with_font<R>(f:impl FnOnce(&mut CachedFont)->R)->R {
    let me=libgfx_pid();
    let mut seen=FONT.owner.load(Ordering::Relaxed);
    let inherited=loop {
        if seen&1==0 || seen>>1!=me {
            match FONT.owner.compare_exchange_weak(seen,me<<1|1,Ordering::Acquire,Ordering::Relaxed) {
                Ok(_)=>break seen>>1!=0 && seen>>1!=me, Err(n)=>seen=n,
            }
        } else { core::hint::spin_loop();seen=FONT.owner.load(Ordering::Relaxed); }
    };
    struct Held(u64); impl Drop for Held {fn drop(&mut self){FONT.owner.store(self.0<<1,Ordering::Release);}}
    let _held=Held(me);
    if inherited { unsafe { FONT.font.get().write(None); } }
    let slot=unsafe{&mut *FONT.font.get()};
    let font=slot.get_or_insert_with(|| CachedFont::with_default_cache(Font::parse(SANS).expect("pinned DejaVu Sans")));
    f(font)
}
fn libgfx_pid()->u64 {
    #[cfg(target_os="breenix")] {libbreenix::process::getpid().map(|p|p.raw()).unwrap_or(0)}
    #[cfg(not(target_os="breenix"))] {unsafe extern "C" {fn getpid()->i32;} unsafe{getpid() as u64}}
}
pub fn text_width(text:&[u8],theme:&Theme)->i32 {
    if theme.use_bitmap_font {bitmap_font::text_width(text) as i32}
    else {with_font(|font|ttf_font::text_width_bytes(font,text,16.0))}
}
pub fn text_height(_theme:&Theme)->i32 {16}
pub fn draw_text(fb:&mut Surface,text:&[u8],x:i32,y:i32,c:Color,theme:&Theme) {
    let s=fb.ui_scale() as i32;
    if theme.use_bitmap_font {
        let height=if s==2 {bitmap_font::RasterHeight::Size32}else{bitmap_font::RasterHeight::Size16};
        let advance=bitmap_font::metrics_at(height).char_width as i32;
        let (x,y)=(x.saturating_mul(s),y.saturating_mul(s));
        for (i,&ch) in text.iter().enumerate() {
            let px=x as i64+i as i64*advance as i64;
            if px>=0 && y>=0 {bitmap_font::draw_char_at(fb,ch as char,px as usize,y as usize,c,height);}
        }
    } else {
        with_font(|font|{ttf_font::draw_text_bytes(fb,font,text,x.saturating_mul(s),y.saturating_mul(s),16.0*s as f32,c);});
    }
}
pub fn draw_text_centered(fb:&mut Surface,text:&[u8],rect:&Rect,c:Color,theme:&Theme) {
    draw_text(fb,text,rect.x+(rect.w-text_width(text,theme))/2,rect.y+(rect.h-16)/2,c,theme);
}
