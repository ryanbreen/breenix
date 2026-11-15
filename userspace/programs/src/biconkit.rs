//! Breenix Icon Kit showcase.
use breengel::{Event, Handled, Window, tree::{Flag, Node, Role, Tree}};
use libgfx::{color::Color, font, shapes};
use libicon::{icon::{Icon, IconState}, icons::{home::HomeIcon, arrow::ArrowIcon, search::SearchIcon, save::SaveIcon}};
#[path = "biconkit/input.rs"] mod input;
use input::{Input, icon_mouse_for, HOVER_RADIUS};
#[path = "biconkit/layout.rs"] mod layout;
use layout::Layout;

fn icons() -> Vec<Box<dyn Icon>> { vec![Box::new(HomeIcon::new()), Box::new(ArrowIcon::new(false)), Box::new(ArrowIcon::new(true)), Box::new(SearchIcon::new()), Box::new(SaveIcon::new())] }
const NAMES: [&str; 5] = ["Home", "Back", "Forward", "Search", "Save"];
fn state(s: IconState) -> &'static str { match s { IconState::Idle => "Idle", IconState::HoverIn => "HoverIn", IconState::Hovering => "Hovering", IconState::HoverOut => "HoverOut", IconState::Pressed => "Pressed", IconState::Clicked => "Clicked" } }
fn main() {
    let ui_scale = libbreenix::gpu::current_mode().expect("display mode").scale;
    assert!((1..=2).contains(&ui_scale), "invalid icon kit UI scale");
    // Publish the scaled size at creation so bwm can place the whole frame.
    let mut win=Window::new_without_fonts(b"Breenix Icon Kit",600 * ui_scale,480 * ui_scale).expect("window");
    let mut grid=icons(); let mut preview=icons(); let mut input=Input::new();
    let mut last=None; let mut epochs=0u64; let mut redraw=true;
    win.set_animating(true);
    loop {
        let layout = Layout::new(None, 7.0, ui_scale, win.width() as i32, win.height() as i32);
        if !redraw {
            let mut close=false; let mut resize=None; let mut serial=false;
            let (preview_x, preview_y) = layout.preview();
            let (preview_x, preview_y) = layout.logical_point(preview_x, preview_y);
            let count=win.wait_dispatch(100_000_000, |d| {
                serial |= d.serial != 0;
                match d.event {
                    Event::CloseRequested => close=true,
                    Event::ResizeRequested {width,height} => { resize=Some((width,height)); redraw=true; },
                    Event::Resized {..} => redraw=true,
                    Event::KeyPress {ascii,..} => { if ascii == b'q' || ascii == b'Q' { close=true; } else if input.navigate(ascii,grid.len()) { preview[input.selected].reset(); redraw=true; } },
                    Event::MouseMove {x,y} => { let (x,y)=layout.logical_point(x,y); input.move_to(x,y,&mut grid); let pm=icon_mouse_for(x,y,input.down,false,false,preview_x,preview_y,60); preview[input.selected].update(1000,pm); redraw=true; },
                    Event::MouseButton {button,pressed,x,y} if button == breengel::buttons::LEFT => {
                        let (x,y)=layout.logical_point(x,y);
                        let old=input.selected; input.button(pressed,x,y,&mut grid); input.tick(1000,&mut grid);
                        if old != input.selected || (!pressed && grid.iter().enumerate().any(|(i,_)| { let (cx,cy)=input::grid_center(i); icon_mouse_for(x,y,false,false,true,cx,cy,HOVER_RADIUS).just_released })) { preview[input.selected].reset(); }
                        let pm=icon_mouse_for(x,y,input.down,pressed,!pressed,preview_x,preview_y,60); preview[input.selected].update(1000,pm); redraw=true;
                    },
                    Event::AnimationTick {time_ms} => { let dt=last.map_or(0,|t:u64| time_ms.saturating_sub(t)).min(1000); last=Some(time_ms); let us=(dt*1000) as u32; input.tick(us,&mut grid); let pm=icon_mouse_for(input.x,input.y,input.down,false,false,preview_x,preview_y,60); preview[input.selected].update(us,pm); epochs=epochs.saturating_add(1); redraw=true; },
                    _ => {}
                }
                if redraw {Handled::Redraw} else {Handled::NoChange}
            }).expect("dispatch");
            if close { return; } if let Some((w,h))=resize { let _=win.apply_resize(w,h); }
            if !redraw { if count != 0 && serial { let _=win.commit_no_change(); } continue; }
        }
        redraw=false;
        let layout = Layout::new(None, 7.0, ui_scale, win.width() as i32, win.height() as i32);
        let bounds=[0,0,win.width() as i64,win.height() as i64];
        let mut children=vec![Node { key:"icons".into(), parent:Some("status".into()), role:Role::List, name:"Icons".into(), value:serde_json::json!({"selected":input.selected,"icons":grid.iter().enumerate().map(|(i,x)| serde_json::json!({"name":NAMES[i],"state":state(x.state())})).collect::<Vec<_>>()}), bounds, clip:bounds, flags:vec![Flag::Enabled,Flag::Visible] }];
        for (i,icon) in grid.iter().enumerate() { let [x,y,w,h]=layout.icon_rect(i); children.push(Node {key:format!("icon-{i}"),parent:Some("icons".into()),role:Role::Button,name:NAMES[i].into(),value:serde_json::json!({"state":state(icon.state())}),bounds:[x as i64,y as i64,w as i64,h as i64],clip:bounds,flags:vec![Flag::Enabled,Flag::Visible]}); }
        win.set_tree(Tree {root:Node {key:"status".into(),parent:None,role:Role::Status,name:"biconkit".into(),value:serde_json::json!({"demo":"biconkit","objects":grid.len(),"speed":100,"epochs":epochs,"inBounds":grid.iter().enumerate().all(|(i,_)| { let [x,y,w,h]=layout.icon_rect(i); x >= 0 && y >= 0 && x+w <= win.width() as i32 && y+h <= win.height() as i32 }),"width":win.width(),"height":win.height(),"uiScale":ui_scale}),bounds,clip:bounds,flags:vec![Flag::Enabled,Flag::Visible]},children});
        let (preview_x,preview_y)=layout.preview();
        let px=|n| layout.text.px(n);
        let scale=layout.text.scale as usize;
        let fb=win.surface().expect("surface"); fb.clear(Color::rgb(30,30,40));
        font::draw_text(fb,b"ICON LIBRARY",px(20) as usize,px(15) as usize,Color::WHITE,2*scale);
        for (i,icon) in grid.iter().enumerate() { let (x,y)=layout.grid_center(i); if i==input.selected { shapes::draw_rect(fb,x-px(25),y-px(25),px(50),px(50),Color::rgb(80,140,220)); } icon.draw(fb,x,y,px(40)); font::draw_text(fb,NAMES[i].as_bytes(),(x-NAMES[i].len() as i32*layout.text.cell_w/2).max(0) as usize,(y+px(20)+layout.text.line_h) as usize,Color::WHITE,scale); }
        font::draw_text(fb,b"INTERACTIVE PREVIEW",px(20) as usize,px(235) as usize,Color::WHITE,2*scale); preview[input.selected].draw(fb,preview_x,preview_y,px(80));
        font::draw_text(fb,state(preview[input.selected].state()).as_bytes(),(preview_x-font::text_width(state(preview[input.selected].state()).as_bytes(),scale) as i32/2).max(0) as usize,(preview_y+px(50)) as usize,Color::WHITE,scale);
        font::draw_text(fb,b", Prev    . Next    Q Quit",px(20) as usize,(layout.height-px(30)).max(0) as usize,Color::WHITE,scale);
        if let Err(error) = win.present() { eprintln!("[biconkit] present failed: {error}"); std::process::exit(1); }
    }
}
