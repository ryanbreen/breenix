//! Logical UI geometry, emitted as physical GPU primitives at the surface's UI scale.
use libgfx::{color::Color, shapes, surface::Surface};
fn mul(v:i32,s:i32)->i32 { v.saturating_mul(s) }
pub fn fill_rect(f:&mut Surface,x:i32,y:i32,w:i32,h:i32,c:Color) { let s=f.ui_scale() as i32; shapes::fill_rect(f,mul(x,s),mul(y,s),mul(w,s),mul(h,s),c); }
pub fn draw_rect(f:&mut Surface,x:i32,y:i32,w:i32,h:i32,c:Color) { let s=f.ui_scale() as i32; for i in 0..s { shapes::draw_rect(f,mul(x,s)+i,mul(y,s)+i,mul(w,s)-2*i,mul(h,s)-2*i,c); } }
pub fn draw_line(f:&mut Surface,x:i32,y:i32,xx:i32,yy:i32,c:Color) { let s=f.ui_scale() as i32; shapes::draw_line(f,mul(x,s),mul(y,s),mul(xx,s),mul(yy,s),c); }
pub fn fill_circle(f:&mut Surface,x:i32,y:i32,r:i32,c:Color) { let s=f.ui_scale() as i32; shapes::fill_circle(f,mul(x,s),mul(y,s),mul(r,s),c); }
pub fn draw_circle(f:&mut Surface,x:i32,y:i32,r:i32,c:Color) { let s=f.ui_scale() as i32; shapes::draw_circle(f,mul(x,s),mul(y,s),mul(r,s),c); }
pub fn push_clip(f:&mut Surface,r:libgfx::surface::Rect) {let s=f.ui_scale() as i32;f.push_clip(libgfx::surface::Rect::new(mul(r.x,s),mul(r.y,s),mul(r.w,s),mul(r.h,s)));}
