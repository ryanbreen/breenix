"""Execute production damage routing for completed gestures and session updates."""
import unittest
from guskit_new_save_test import ROOT, run_host
from gt710_gr_live_unwind_test import item

class EventDamage(unittest.TestCase):
    def test_events_commit_bounded_damage_covering_all_changed_pixels(self):
        s=(ROOT/'userspace/programs/src/guskit.rs').read_text()
        methods='\n'.join(item(s,f'    fn {name}(') for name in ['content_damage'] if f'fn {name}(' in s)
        helpers='\n'.join(item(s,f'fn {name}(') for name in ['union','dispatch_input','invalidate'] if f'fn {name}(' in s)
        main=s[s.index('fn main()'):]
        poll=item(main,'if let Some(session)=&mut app.session {')
        # Include the post-poll damage publication, if present.
        if 'if session_changed {' in main: poll += '\n'+item(main,'if session_changed {')
        run_host(self,r'''
#[cfg(test)] mod check {
#[allow(dead_code)] #[path="CANVAS"] mod canvas;
use canvas::Canvas;
#[derive(Clone,Copy,Debug)] struct Clip {x:i32,y:i32,w:i32,h:i32}
impl Clip {fn new(x:i32,y:i32,w:i32,h:i32)->Self {Self{x,y,w,h}}}
#[derive(Clone,Copy)] struct Layout {scale:i32,canvas_y:i32,status:Clip}
impl Layout {fn canvas(&self,w:usize,h:usize)->Clip {Clip::new(0,self.canvas_y,w as i32*self.scale,h as i32*self.scale)}}
struct Model {status:String,canvas:Canvas}
impl Model {fn width(&self)->usize {self.canvas.dimensions().0} fn height(&self)->usize {self.canvas.dimensions().1}}
struct Core {model:Model} struct Session {core:Core}
impl Session {fn poll(&mut self,_:i32)->bool {true}}
struct App {layout:Layout,damage:Option<Clip>,width:usize,height:usize,image:Canvas,old:Canvas,
 canvas_surface:Option<(Canvas,())>,stroke:Option<Clip>,picker:Option<()>,session_dialog:Option<()>,confirm:bool,
 session:Option<Session>,message:String,tool:usize,size:usize,hue:u16,sat:u8,val:u8,selected_color:Option<[u8;3]>,recent:[[u8;3];8]}
impl App {
 fn canvas(&self)->&Canvas {self.session.as_ref().map_or(&self.image,|s|&s.core.model.canvas)}
 fn stroke_bounds(&self)->Option<Clip> {self.stroke}
 METHODS
}
struct MenuBar;
impl MenuBar {fn is_open(&self)->bool {false}}
#[derive(Clone,Copy)] enum Event {MouseMove{x:i32,y:i32},Start,End,Undo,RefusedUndo,Palette,PointerCancel,Idle}
#[derive(PartialEq,Debug)] enum Handled {Redraw,NoChange}
mod breengel {pub struct Delivered {pub event:super::Event}}
fn handle(app:&mut App,_:&mut MenuBar,d:&breengel::Delivered)->Handled {
 match d.event {
  Event::Start=>app.stroke=Some(Clip::new(10,80,6,5)),
  Event::MouseMove{x,y}=>app.stroke=Some(Clip::new(x,y,65,5)),
  Event::End=>{app.old=app.image.clone();app.image.pencil(&[(10,80),(70,80)],[1,2,3]);app.stroke=None;},
  Event::Undo=>app.image=app.old.clone(),
  Event::PointerCancel=>app.stroke=None,
  Event::Idle=>return Handled::NoChange,
  Event::Palette=>app.hue=120,
  Event::RefusedUndo=>{},
 }
 Handled::Redraw
}
HELPERS
#[test] fn event_damage() {
 let base=Canvas::new(1148,454);
 let mut app=App{layout:Layout{scale:2,canvas_y:352,status:Clip::new(220,300,1380,52)},damage:None,width:1148,height:454,
  image:base.clone(),old:base.clone(),canvas_surface:Some((base,())),stroke:None,picker:None,session_dialog:None,
  confirm:false,session:None,message:String::new(),tool:0,size:0,hue:0,sat:0,val:0,selected_color:None,recent:[[0;3];8]};
 for event in [Event::Start,Event::MouseMove{x:10,y:80},Event::End,Event::Undo,Event::RefusedUndo,Event::Palette,Event::Start,Event::PointerCancel] {
  let before=app.canvas().clone();let mut redraw=false;app.damage=None;
  assert_eq!(dispatch_input(&mut app,&mut MenuBar,&breengel::Delivered{event},&mut redraw),Handled::Redraw);
  verify(&app,&before,redraw);
  app.canvas_surface=Some((app.canvas().clone(),()));
 }
 // A smaller host document must erase the previously visible larger image.
 app.image.pencil(&[(1000,400)],[9,8,7]);app.canvas_surface=Some((app.image.clone(),()));
 app.session=Some(Session{core:Core{model:Model{status:String::new(),canvas:Canvas::new(200,120)}}});
 let r=app.content_damage();
 assert!(r.x<=2000 && r.y<=1152 && r.x+r.w>=2002 && r.y+r.h>=1154,"old document extent was not erased");
 app.session=None;app.image=Canvas::new(1148,454);app.canvas_surface=Some((app.image.clone(),()));
 let mut redraw=false;assert_eq!(dispatch_input(&mut app,&mut MenuBar,&breengel::Delivered{event:Event::Idle},&mut redraw),Handled::NoChange);
 for undo in [false,true] {
  let before=app.canvas().clone();let mut changed=before.clone();
  if undo {changed=app.old.clone()} else {changed.pencil(&[(10,80),(70,80)],[40,204,40])}
  app.session=Some(Session{core:Core{model:Model{status:String::new(),canvas:changed}}});
  let mut redraw=false;app.damage=None;let menu=MenuBar;
  SESSION_CHANGED
  POLL
  verify(&app,&before,redraw);app.canvas_surface=Some((app.canvas().clone(),()));
 }
}
fn verify(app:&App,before:&Canvas,redraw:bool) {
 assert!(app.canvas_surface.is_some());assert_eq!(app.layout.canvas(app.width,app.height).w,2296);
 assert!(redraw);let r=app.damage.expect("event widened damage to the full 2296x1260 window");
 let pixels=r.w as usize*r.h as usize;
 println!("event damage {}x{} = {} pixels vs full {}",r.w,r.h,pixels,2296*1260);
 assert!(pixels<800_000,"event still scales with the full window: {r:?}");
 app.canvas().render_changes(before,|x,y,w,_|{
  let x=x as i32*2;let y=y as i32*2+352;
  assert!(r.x<=x&&r.y<=y&&r.x+r.w>=x+w as i32*2&&r.y+r.h>=y+2,"changed pixel omitted from {r:?}");
 });
}
}
'''.replace('CANVAS',str(ROOT/'userspace/programs/src/guskit/canvas.rs')).replace('METHODS',methods).replace('HELPERS',helpers).replace('SESSION_CHANGED','let mut session_changed=false;' if 'if session_changed {' in main else '').replace('POLL',poll))
if __name__=='__main__':unittest.main()
