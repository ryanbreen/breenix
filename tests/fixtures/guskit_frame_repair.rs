// The syscall boundary: ACQUIRE has already repaired the selected back buffer
// from the current front (kernel/src/window/ops.rs::acquire). COMMIT advertises
// only its supplied delta to bwm; C is persistent between composites.
struct Acquired { replay_full:bool, replay:Vec<libbreenix::window::abi::Rect> }
struct Window {
    buffers:[Surface;2], used:[bool;2], front:Option<usize>, back:usize,
    scene:Surface, damage:Clip,
}
impl Window {
    fn width(&self)->u32 {1148}
    fn height(&self)->u32 {831}
    fn acquire(&mut self,_:u64)->Result<Acquired,String> {
        self.back=self.front.map_or(0,|front|front^1);
        let full=!self.used[self.back];
        let repair=if full {Clip::new(0,0,1148,831)} else {self.damage};
        if let Some(front)=self.front {
            for y in repair.y..repair.y+repair.h {for x in repair.x..repair.x+repair.w {
                let i=y as usize*1148+x as usize;
                self.buffers[self.back].pixels[i]=self.buffers[front].pixels[i];
            }}
        }
        Ok(Acquired {replay_full:full,replay:vec![libbreenix::window::abi::Rect::new(repair.x,repair.y,repair.w,repair.h)]})
    }
    fn surface(&mut self)->Result<&mut Surface,String> {Ok(&mut self.buffers[self.back])}
    fn commit(&mut self,damage:&[libbreenix::window::abi::Rect])->Result<(),String> {
        let r=damage[0]; self.damage=Clip::new(r.x,r.y,r.w,r.h);
        self.front=Some(self.back); self.used[self.back]=true;
        for y in r.y..r.y+r.h {for x in r.x..r.x+r.w {
            let i=y as usize*1148+x as usize;
            self.scene.pixels[i]=self.buffers[self.back].pixels[i];
        }}
        Ok(())
    }
}
fn publish(_: &mut Window,_:&App,_:&MenuBar,_:f32) {}
#[test]
fn join_replay_then_raise_keeps_the_scanout_equal_to_the_committed_front() {
    let surface=||Surface::new(1148,831,libgfx::surface::Format::BGRX).unwrap();
    let mut win=Window {buffers:[surface(),surface()],used:[false;2],front:None,back:0,
        scene:surface(),damage:Clip::new(0,0,1148,831)};
    // Core::join starts at 800x424; A.7's initial view is 1148x831.
    let mut app=App {image:Canvas::new(800,424),width:1148,height:655,
        layout:Layout::new(1),damage:None,tool:0,size:0,hue:0,sat:0,val:0,
        recent:[[0;3];8],stroke:None,selected_color:None,palette:None,canvas_surface:None,
        picker:None,confirm:false,session:None,session_dialog:None,message:String::new(),
        file:None,directory:PathBuf::from("."),save_as:false};
    present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0).unwrap();
    // Welcome/SyncEnd replaces the provisional join image with the host's 200x120.
    // This is the main loop's post-poll dimension update and damage publication.
    app.image=Canvas::new(200,120);app.width=200;app.height=120;
    app.damage=Some(app.content_damage());
    present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0).unwrap();
    // Raising a window makes bwm redraw its full footprint from its latest front.
    win.scene.pixels.clone_from(&win.buffers[win.front.unwrap()].pixels);
    // A later metadata/status redraw reuses the older buffer with a delta replay.
    app.damage=Some(app.content_damage());
    present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0).unwrap();
    let i=176*1148+1103; // retained capture (1185,298), client origin (82,122)
    assert_eq!(win.scene.pixels[i],Color::rgb(45,45,45));
    assert_eq!(win.scene.pixels[i],win.buffers[win.front.unwrap()].pixels[i],
        "screen (1185,298) must match the committed client pixel (1103,176)");
    let mismatch=win.scene.pixels.iter().zip(&win.buffers[win.front.unwrap()].pixels)
        .position(|(shown,front)|shown!=front);
    assert_eq!(mismatch,None,"bwm's persistent scene must equal the whole committed front");
}
