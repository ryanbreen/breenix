//! Bouncing rectangles demo.
use breengel::{Event, Handled, Window};
use libgfx::{color::Color, shapes};
#[path = "demos/status.rs"] mod status;
const NUM_RECTS: usize = 6;
struct AnimRect { x: i32, y: i32, vx: i32, vy: i32, w: i32, h: i32, color: Color }
impl AnimRect {
 fn new(x:i32,y:i32,vx:i32,vy:i32,w:i32,h:i32,color:Color)->Self { Self{x:x*100,y:y*100,vx,vy,w,h,color} }
 fn clamp(&mut self, width:i32,height:i32) { let max_x = (width-self.w.min(width)).max(0)*100; let max_y = (height-self.h.min(height)).max(0)*100; if self.x < 0 || self.x > max_x { self.vx = -self.vx; } if self.y < 0 || self.y > max_y { self.vy = -self.vy; } self.x = self.x.clamp(0,max_x); self.y = self.y.clamp(0,max_y); }
}
fn make_rects() -> [AnimRect; NUM_RECTS] {
    [
        AnimRect::new( 50,  40,  900,  700,  120,  80, Color::rgb(255,  50,  50)),  // Red
        AnimRect::new(200, 150, -800,  600,   90, 110, Color::rgb( 50, 255,  50)),  // Green
        AnimRect::new(100, 300,  750, -850,  140,  60, Color::rgb( 50,  50, 255)),  // Blue
        AnimRect::new(300, 100, -700, -750,   70, 130, Color::rgb(255, 255,  50)),  // Yellow
        AnimRect::new(150, 400,  850,  500,  100, 100, Color::rgb(255,  50, 255)),  // Magenta
        AnimRect::new(350, 250, -650,  800,  110,  70, Color::rgb( 50, 255, 255)),  // Cyan
    ]
}


fn main() {
 let mut win = Window::new_without_fonts(b"Rectangles",400,300).expect("window"); let mut rects = make_rects();
 let mut last = None; let mut remainder = 0u64; let mut epochs = 0u64; let mut redraw = true;
 win.set_animating(true);
 loop {
  if !redraw {
   let mut close=false; let mut resize=None; let mut input=false; let width=win.width() as i32; let height=win.height() as i32;
   let count=win.wait_dispatch(100_000_000, |d| { input |= d.serial != 0; match d.event {
    Event::AnimationTick { time_ms } => { let elapsed=remainder+last.map_or(0,|t:u64| time_ms.saturating_sub(t)); last=Some(time_ms); remainder=elapsed%16; for _ in 0..(elapsed/16).min(1000) { for r in &mut rects { r.x+=r.vx; r.y+=r.vy; r.clamp(width,height); } epochs=epochs.saturating_add(1); } redraw=true; },
    Event::MouseMove { .. } => redraw = true,
    Event::CloseRequested => close=true,
    Event::ResizeRequested { width,height } => { resize=Some((width,height)); redraw=true; },
    Event::Resized { .. } => redraw=true, _ => {} }
    if redraw { Handled::Redraw } else { Handled::NoChange }
   }).expect("dispatch");
   if close { return; } if let Some((w,h))=resize { let _=win.apply_resize(w,h); }
   if !redraw { if count != 0 && input { let _=win.commit_no_change(); } continue; }
  }
  redraw=false; let width=win.width() as i32; let height=win.height() as i32;
  for r in &mut rects { r.clamp(width,height); }
  let objects:Vec<_>=rects.iter().map(|r| status::Object { bounds:[(r.x/100) as i64,(r.y/100) as i64,r.w.min(width) as i64,r.h.min(height) as i64], value:serde_json::json!({}) }).collect();
  let inside = objects.iter().all(|o| o.bounds[0] >= 0 && o.bounds[1] >= 0 && o.bounds[0]+o.bounds[2] <= width as i64 && o.bounds[1]+o.bounds[3] <= height as i64);
  win.set_tree(status::build("rectangles",100,epochs,inside,win.width(),win.height(),"rectangle",&objects));
  let fb=win.surface().expect("surface"); fb.clear(Color::rgb(15,15,30));
  for r in &rects { shapes::fill_rect(fb,r.x/100,r.y/100,r.w.min(width),r.h.min(height),r.color); }
  if let Err(error) = win.present() { eprintln!("[rectangles] present failed: {error}"); std::process::exit(1); }
 }
}
