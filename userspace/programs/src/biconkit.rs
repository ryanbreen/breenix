//! Breenix Icon Kit showcase.
use breengel::{Event, Handled, Window, tree::{Flag, Node, Role, Tree}};
use libgfx::{color::Color, font, shapes};
use libicon::{icon::{Icon, IconState}, icons::{home::HomeIcon, arrow::ArrowIcon, search::SearchIcon, save::SaveIcon}};
#[path = "biconkit/input.rs"] mod input;
use input::{Input, grid_center, icon_mouse_for, HOVER_RADIUS};

fn icons() -> Vec<Box<dyn Icon>> { vec![Box::new(HomeIcon::new()), Box::new(ArrowIcon::new(false)), Box::new(ArrowIcon::new(true)), Box::new(SearchIcon::new()), Box::new(SaveIcon::new())] }
const NAMES: [&str; 5] = ["Home", "Back", "Forward", "Search", "Save"];
fn state(s: IconState) -> &'static str { match s { IconState::Idle => "Idle", IconState::HoverIn => "HoverIn", IconState::Hovering => "Hovering", IconState::HoverOut => "HoverOut", IconState::Pressed => "Pressed", IconState::Clicked => "Clicked" } }
fn main() {
    let mut win=Window::new_without_fonts(b"Breenix Icon Kit",600,480).expect("window");
    let mut grid=icons(); let mut preview=icons(); let mut input=Input::new();
    let mut last=None; let mut epochs=0u64; let mut redraw=true;
    win.set_animating(true);
    loop {
        if !redraw {
            let mut close=false; let mut resize=None; let mut serial=false;
            let preview_x=(win.width() / 2) as i32; let preview_y=(win.height() as i32 - 155).max(0);
            let count=win.wait_dispatch(100_000_000, |d| {
                serial |= d.serial != 0;
                match d.event {
                    Event::CloseRequested => close=true,
                    Event::ResizeRequested {width,height} => { resize=Some((width,height)); redraw=true; },
                    Event::Resized {..} => redraw=true,
                    Event::KeyPress {ascii,..} => { if ascii == b'q' || ascii == b'Q' { close=true; } else if input.navigate(ascii,grid.len()) { preview[input.selected].reset(); redraw=true; } },
                    Event::MouseMove {x,y} => { input.move_to(x,y,&mut grid); let pm=icon_mouse_for(x,y,input.down,false,false,preview_x,preview_y,60); preview[input.selected].update(1000,pm); redraw=true; },
                    Event::MouseButton {button,pressed,x,y} if button == breengel::buttons::LEFT => {
                        let old=input.selected; input.button(pressed,x,y,&mut grid); input.tick(1000,&mut grid);
                        if old != input.selected || (!pressed && grid.iter().enumerate().any(|(i,_)| { let (cx,cy)=grid_center(i); icon_mouse_for(x,y,false,false,true,cx,cy,HOVER_RADIUS).just_released })) { preview[input.selected].reset(); }
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
        let bounds=[0,0,win.width() as i64,win.height() as i64];
        let mut children=vec![Node { key:"icons".into(), parent:Some("status".into()), role:Role::List, name:"Icons".into(), value:serde_json::json!({"selected":input.selected,"icons":grid.iter().enumerate().map(|(i,x)| serde_json::json!({"name":NAMES[i],"state":state(x.state())})).collect::<Vec<_>>()}), bounds, clip:bounds, flags:vec![Flag::Enabled,Flag::Visible] }];
        for (i,icon) in grid.iter().enumerate() { let (x,y)=grid_center(i); children.push(Node {key:format!("icon-{i}"),parent:Some("icons".into()),role:Role::Button,name:NAMES[i].into(),value:serde_json::json!({"state":state(icon.state())}),bounds:[(x-HOVER_RADIUS) as i64,(y-HOVER_RADIUS) as i64,(2*HOVER_RADIUS) as i64,(2*HOVER_RADIUS) as i64],clip:bounds,flags:vec![Flag::Enabled,Flag::Visible]}); }
        win.set_tree(Tree {root:Node {key:"status".into(),parent:None,role:Role::Status,name:"biconkit".into(),value:serde_json::json!({"demo":"biconkit","objects":grid.len(),"speed":100,"epochs":epochs,"inBounds":grid.iter().enumerate().all(|(i,_)| { let (x,y)=grid_center(i); x-HOVER_RADIUS >= 0 && y-HOVER_RADIUS >= 0 && x+HOVER_RADIUS <= win.width() as i32 && y+HOVER_RADIUS <= win.height() as i32 }),"width":win.width(),"height":win.height()}),bounds,clip:bounds,flags:vec![Flag::Enabled,Flag::Visible]},children});
        let preview_x=(win.width()/2) as i32; let preview_y=(win.height() as i32-155).max(0);
        let fb=win.surface().expect("surface"); fb.clear(Color::rgb(30,30,40));
        font::draw_text(fb,b"ICON LIBRARY",20,15,Color::WHITE,2);
        for (i,icon) in grid.iter().enumerate() { let (x,y)=grid_center(i); if i==input.selected { shapes::draw_rect(fb,x-25,y-25,50,50,Color::rgb(80,140,220)); } icon.draw(fb,x,y,40); font::draw_text(fb,NAMES[i].as_bytes(),(x-20) as usize,(y+28) as usize,Color::WHITE,1); }
        font::draw_text(fb,b"INTERACTIVE PREVIEW",20,235,Color::WHITE,2); preview[input.selected].draw(fb,preview_x,preview_y,80);
        font::draw_text(fb,state(preview[input.selected].state()).as_bytes(),(preview_x-30).max(0) as usize,(preview_y+50) as usize,Color::WHITE,1);
        font::draw_text(fb,b", Prev    . Next    Q Quit",20,450,Color::WHITE,1);
        if let Err(error) = win.present() { eprintln!("[biconkit] present failed: {error}"); std::process::exit(1); }
    }
}
