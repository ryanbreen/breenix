#![cfg(test)]
// Extracted versions use different subsets of the recorder API and model fields.
#![allow(dead_code)]
// The fixture records app GPU calls; libbui Rect, text sizing and semantic nodes
// are production code. Document/session I/O is outside this layout check.
use libbui::{Rect, Theme};
use libgfx_real::surface::Rect as Clip;
use libgfx_real::bitmap_font::RasterHeight;
use std::path::PathBuf;
extern crate libgfx as libgfx_real;
mod breengel { pub mod tree { pub use libbui::tree::*; } }
mod libgfx {
    pub use libgfx_real::surface;
    pub mod blit {
        pub fn blit_scaled(dst: &mut super::super::Surface, src: &super::super::Surface, x:i32, y:i32, n:i32) {
            dst.rects.push([x as i64,y as i64,src.width as i64*n as i64,src.height as i64*n as i64]);
            for sy in 0..src.height { for sx in 0..src.width { for dy in 0..n { for dx in 0..n {
                dst.put_pixel((x+sx as i32*n+dx) as usize,(y+sy as i32*n+dy) as usize,src.pixels[sy*src.width+sx]);
            }}}}
        }

        pub fn blit(dst: &mut super::super::Surface, src: &super::super::Surface, x: i32, y: i32) {
            dst.rects.push([x as i64, y as i64, src.width as i64, src.height as i64]);
            for sy in 0..src.height { for sx in 0..src.width {
                dst.put_pixel(sx + x as usize, sy + y as usize, src.pixels[sy*src.width+sx]);
            }}
        }
    }
}
use libgfx_real::color::Color;
struct Surface {
    width:usize, height:usize, scale:u32, pixels:Vec<Color>, rects:Vec<[i64;4]>,
    points:Vec<(usize,usize)>, clips:Vec<Clip>, labels:Vec<(String,i32,i32,f32,Clip)>,
}
impl Surface {
    fn new(width:usize,height:usize,_:libgfx::surface::Format)->Result<Self,String> {
        Ok(Self { width,height,scale:1,pixels:vec![Color::WHITE;width*height],rects:vec![],points:vec![],clips:vec![],labels:vec![] })
    }
    fn ui_scale(&self)->u32 {self.scale}
    fn width(&self)->usize {self.width}
    fn height(&self)->usize {self.height}
    fn batch<R>(&mut self,f:impl FnOnce(&mut Self)->R)->R {f(self)}
    fn clear(&mut self,c:Color) {self.pixels.fill(c)}
    fn put_pixel(&mut self,x:usize,y:usize,c:Color) {self.points.push((x,y));if x<self.width && y<self.height {self.pixels[y*self.width+x]=c}}
    fn wait(&self)->Result<(),String> {Ok(())}
    fn push_clip(&mut self,r:Clip) {self.clips.push(r)}
    fn pop_clip(&mut self) {self.clips.pop();}
}
mod shapes {
    pub fn fill_rect(f:&mut super::Surface,x:i32,y:i32,w:i32,h:i32,c:super::Color) {
        f.rects.push([x as i64,y as i64,w as i64,h as i64]);
        for py in y.max(0)..(y+h).min(f.height as i32) {for px in x.max(0)..(x+w).min(f.width as i32) {f.pixels[py as usize*f.width+px as usize]=c}}
    }
}
struct CachedFont;
mod ttf_font {
    pub fn draw_text(f:&mut super::Surface,_:&mut super::CachedFont,t:&str,x:i32,y:i32,size:f32,_:super::Color) {
        f.labels.push((t.into(),x,y,size,*f.clips.last().unwrap()));
    }
}
mod bitmap_font {
    pub fn draw_text(f:&mut super::Surface,t:&[u8],x:usize,y:usize,c:super::Color) {draw_text_at(f,t,x,y,c,super::RasterHeight::Size16)}
    pub fn draw_text_at(f:&mut super::Surface,t:&[u8],x:usize,y:usize,_:super::Color,r:super::RasterHeight) {
        let size=if matches!(r,super::RasterHeight::Size32) {32.0} else {16.0};
        f.labels.push((String::from_utf8_lossy(t).into_owned(),x as i32,y as i32,size,*f.clips.last().unwrap()));
    }
}
struct MenuBar;
impl MenuBar { fn draw(&self,_:&mut Surface,_:&Theme) {} }
impl libbui::widget::Widget for MenuBar {
    type Event=();
    fn rect(&self)->Rect {Rect::new(0,0,800,22)}
    fn draw(&self,_:&mut libgfx_real::surface::Surface,_:&Theme) {}
    fn update(&mut self,_:&libbui::InputState) {}
    fn tree_nodes(&self,_:&str,_:Option<&str>,_:Rect,_:&Theme)->Vec<libbui::tree::Node> {vec![]}
}
struct Field { rect:Rect }
impl Field {
    fn draw(&self,f:&mut Surface,_:&Theme) {let s=f.scale as i32;shapes::fill_rect(f,self.rect.x*s,self.rect.y*s,self.rect.w*s,self.rect.h*s,Color::WHITE)}
    fn text(&self)->&str {"Peer"}
    fn is_focused(&self)->bool {true}
}
struct Picker { rect:Rect }
impl Picker {fn draw(&self,f:&mut Surface,t:&Theme) {Field{rect:self.rect}.draw(f,t)}}
mod session_dialog { #[derive(PartialEq,Eq)] pub enum Mode {Host,Join} }
struct DialogModel;
impl DialogModel {fn status(&self)->&str {""}}
struct SessionDialog {mode:session_dialog::Mode,address:Field,name:Field,model:DialogModel}
#[derive(Clone)] struct Canvas;
impl Canvas {
    fn region(&self,_:usize,_:usize,_:usize,_:usize)->Self {Self}
    fn preview(&self,p:&[(i32,i32)],f:impl FnOnce(&mut Self,&[(i32,i32)]))->Self {let mut c=Self;f(&mut c,p);c}
    fn render_runs(&self,_:impl FnMut(usize,usize,usize,[u8;3])) {}
    fn render_changes(&self,_:&Self,_:impl FnMut(usize,usize,usize,[u8;3]))->bool {false}
    fn digest(&self)->[u8;32] {[0;32]}
}
struct Model {name:String,state:String}
impl node::Collaboration for Model {fn collab_value(&self)->serde_json::Value {serde_json::Value::Null}}
struct Core {model:Model}
struct Session {core:Core}
struct App {
    width:usize,height:usize,tool:usize,size:usize,hue:u16,sat:u8,val:u8,recent:[[u8;3];8],
    stroke:Option<Vec<(i32,i32)>>, selected_color:Option<[u8;3]>, palette:Option<(u16,Surface)>,
    canvas_surface:Option<(Canvas,Surface)>,picker:Option<Picker>,confirm:bool,
    session:Option<Session>,session_dialog:Option<SessionDialog>,message:String,
    file:Option<PathBuf>,directory:PathBuf,save_as:bool,
    // APP_LAYOUT_FIELD
}
type Point=(i32,i32);
impl App {
    fn canvas(&self)->&Canvas {&Canvas}
    fn draw_stroke(&self,_:&mut Canvas,_:&[Point]) {}
    fn status(&self)->&str {&self.message}
    fn color(&self)->[u8;3] {self.selected_color.unwrap_or_else(||node::hsv_to_rgb(self.hue,self.sat,self.val))}
    fn undo_depth(&self)->usize {0}
    fn redo_depth(&self)->usize {0}
    fn dirty(&self)->bool {false}
    // APP_METHODS
}
struct Window {scale:u32,tree:Option<libbui::tree::Tree>}
impl Window {
    fn width(&self)->u32 {800*self.scale}
    fn height(&self)->u32 {600*self.scale}
    fn display_path(&self)->&str {"font.ttf"}
    fn set_tree(&mut self,t:libbui::tree::Tree) {self.tree=Some(t)}
}
mod node {
    // NODE_MODULE
}
// CONSTANTS
// LAYOUT_MODULE
// PRODUCTION
fn sample(scale:u32,ttf:bool)->(App,Surface,libbui::tree::Tree) {
    let mut app=App {width:200,height:120,tool:0,size:0,hue:120,sat:0,val:0,recent:[[0;3];8],stroke:None,selected_color:None,
        palette:None,canvas_surface:None,picker:None,confirm:true,session:None,
        session_dialog:Some(SessionDialog{mode:session_dialog::Mode::Join,address:Field{rect:Rect::new(270,250,300,24)},name:Field{rect:Rect::new(270,300,300,24)},model:DialogModel}),
        message:"Ready".into(),file:None,directory:PathBuf::from("."),save_as:false,
        // APP_LAYOUT_INIT
    };
    let mut fb=Surface::new((800*scale) as usize,(600*scale) as usize,libgfx::surface::Format::BGRX).unwrap();fb.scale=scale;
    let mut font=ttf.then_some(CachedFont);
    render(&mut fb,&mut app,&MenuBar,&mut font,14.0);
    let mut win=Window{scale,tree:None};publish(&mut win,&app,&MenuBar,14.0);
    // File picker is modal, but must share the same scaling contract.
    app.session_dialog=None;app.confirm=false;app.picker=Some(Picker{rect:Rect::new(150,100,500,400)});
    render(&mut fb,&mut app,&MenuBar,&mut font,14.0);publish(&mut win,&app,&MenuBar,14.0);
    let picker=win.tree.take().unwrap().children.into_iter().find(|n|n.key=="open").unwrap();
    app.picker=None;app.confirm=true;
    app.session_dialog=Some(SessionDialog{mode:session_dialog::Mode::Join,address:Field{rect:Rect::new(270,250,300,24)},name:Field{rect:Rect::new(270,300,300,24)},model:DialogModel});
    publish(&mut win,&app,&MenuBar,14.0);let mut tree=win.tree.unwrap();tree.children.push(picker);
    (app,fb,tree)
}
#[test]
fn scaled_drawing_publication_and_tool_centres() {
    for ttf in [true,false] {
        let (_,one,t1)=sample(1,ttf);let (mut app,two,t2)=sample(2,ttf);
        assert_eq!(one.rects.len(),two.rects.len());
        for (a,b) in one.rects.iter().zip(&two.rects) {
            let expected=a.map(|v|v*2);
            assert_eq!(*b,expected,"drawn rectangle must follow UI scale");
        }
        for (a,b) in one.labels.iter().zip(&two.labels) {
            assert_eq!((&b.0,b.1,b.2,b.3,b.4),(&a.0,a.1*2,a.2*2,a.3*2.0,Clip::new(a.4.x*2,a.4.y*2,a.4.w*2,a.4.h*2)),"text and clip must scale");
        }
        for n in &t2.children {
            let original=t1.children.iter().find(|a|a.key==n.key).unwrap();
            let expected=original.bounds.map(|v|v*2);
            assert_eq!(n.bounds,expected,"{} semantic scale",n.key);
            if n.key!="hue" && n.key!="sv" {
                assert!(two.rects.contains(&n.bounds) || two.labels.iter().any(|l|[l.4.x as i64,l.4.y as i64,l.4.w as i64,l.4.h as i64]==n.bounds),"{} publication {:?} must equal drawn bounds",n.key,n.bounds);
            }
        }
        let palette=&app.palette.as_ref().unwrap().1;
        let bar=[0,0,palette.rects.iter().map(|r|r[0]+r[2]).max().unwrap(),palette.rects.iter().map(|r|r[1]+r[3]).max().unwrap()];
        let square=[palette.points.iter().map(|p|p.0).min().unwrap() as i64,
            palette.points.iter().map(|p|p.1).min().unwrap() as i64,
            (palette.points.iter().map(|p|p.0).max().unwrap()+1) as i64,
            (palette.points.iter().map(|p|p.1).max().unwrap()+1) as i64];
        let hue=t2.children.iter().find(|n|n.key=="hue").unwrap();
        let sv=t2.children.iter().find(|n|n.key=="sv").unwrap();
        assert_eq!(hue.bounds,[bar[0],116+bar[1],bar[2]-bar[0],bar[3]-bar[1]],"hue publishes drawn stripe extent");
        assert_eq!(sv.bounds,[square[0],116+square[1],square[2]-square[0],square[3]-square[1]],"SV publishes written pixel extent");
        for (i,name) in NAMES.iter().enumerate() {
            let n=t2.children.iter().find(|n|n.key==format!("tool-{name}")).unwrap();
            app.tool=(i+1)%7;app.press((n.bounds[0]+n.bounds[2]/2) as i32,(n.bounds[1]+n.bounds[3]/2) as i32);
            assert_eq!(app.tool,i,"scaled tool centre selects {name}");
        }
        assert_eq!(app.point(35,352+39),Some((17,19)));
        assert_eq!(app.point(17,351),None);
        app.press(520,124);app.press(160,152+39);
        assert_eq!(app.color(),[40,204,40],"scaled colour target keeps the same HSV colour");
        assert_eq!(two.pixels[124*two.width+520],Color::rgb(0,255,0));
        assert_eq!(two.pixels[191*two.width+160],Color::rgb(40,204,40));
    }
}
