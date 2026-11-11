//! Gus Kit drawing window.
use breengel::{Color, Event, Handled, Surface, Window};
use libbui::{InputState, Rect, Theme, shortcut::Shortcut, widget::menu_bar::{Menu, MenuBar, MenuEvent, MenuItem, MENU_BAR_HEIGHT}};
use libgfx::{bitmap_font, shapes};
use libbui::widget::file_picker::{FileEntry, FilePicker, FilePickerResult};
use std::path::{Path, PathBuf};

#[path = "guskit/canvas.rs"] mod canvas;
#[path = "guskit/node.rs"] mod node;
#[path = "guskit/files.rs"] mod files;
use canvas::{Canvas, History, Point, Radius};

const TOP: i32 = 176;
const NAMES: [&str; 7] = ["pencil", "brush", "line", "rectangle", "circle", "fill", "eraser"];
const LABELS: [&[u8]; 7] = [b"Pen", b"Brsh", b"Line", b"Rect", b"Circ", b"Fill", b"Ers"];
const NEW: u16 = 1;
const QUIT: u16 = 2;
const UNDO: u16 = 3;
const REDO: u16 = 4;
const OPEN: u16 = 5;
const SAVE: u16 = 6;

struct App {
    width: usize, height: usize, history: History, clean: [u8; 32],
    tool: usize, size: usize, hue: u16, sat: u8, val: u8,
    stroke: Option<Vec<Point>>, selected_color: Option<[u8; 3]>, recent: [[u8; 3]; 8], confirm: bool, quit: bool,
    file: Option<PathBuf>, message: String, picker: Option<FilePicker>, directory: PathBuf,
    palette: Option<(u16, Surface)>,
    canvas_surface: Option<(Canvas, Surface)>,
}
impl App {
    fn color(&self) -> [u8; 3] { self.selected_color.unwrap_or_else(|| node::hsv_to_rgb(self.hue, self.sat, self.val)) }
    fn dirty(&self) -> bool { self.history.canvas().digest() != self.clean }
    fn request_quit(&mut self) { if self.dirty() { self.confirm = true; } else { self.quit = true; } }
    fn action(&mut self, action: u16) {
        match action {
            OPEN => self.open_picker(),
            SAVE => self.save(),
            NEW => { self.history.push(Canvas::new(self.width, self.height)); self.file = None; self.stroke = None; self.message.clear(); },
            QUIT => self.request_quit(),
            UNDO => { self.history.undo(); },
            REDO => { self.history.redo(); },
            _ => {}
        }
    }
    fn open_file(&mut self, path: PathBuf) {
        let result = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| files::decode(&path.to_string_lossy(), &bytes, self.width, self.height));
        match result {
            Ok((image, message)) => { self.clean = image.digest(); self.history.push(image); self.file = Some(path); self.message = message.unwrap_or_default(); },
            Err(e) => self.message = e,
        }
    }
    fn save(&mut self) {
        let mut path = self.file.clone().unwrap_or_default();
        let result = if self.file.is_some() {
            files::encode(&path.to_string_lossy(), self.history.canvas(), self.width, self.height)
                .and_then(|bytes| match libbreenix::atomic_file::save(&path, &bytes) {
                    libbreenix::atomic_file::SaveResult::Saved => Ok(()),
                    libbreenix::atomic_file::SaveResult::Failed(e) | libbreenix::atomic_file::SaveResult::Unconfirmed(e) => Err(e.to_string()),
                })
        } else {
            // Reserve the first free name atomically; another drawing cannot be
            // replaced even if it appears between choosing a name and saving.
            files::encode("untitled.png", self.history.canvas(), self.width, self.height).and_then(|bytes| {
                use std::io::Write;
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                for n in 1u64.. {
                    path = home.join(format!("untitled-{n}.png"));
                    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                        Ok(file) => file,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e.to_string()),
                    };
                    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
                    drop(file);
                    if let Err(e) = result { let _ = std::fs::remove_file(&path); return Err(e.to_string()); }
                    return std::fs::File::open(&home).and_then(|dir| dir.sync_all()).map_err(|e| e.to_string());
                }
                Err("no free untitled filename".into())
            })
        };
        match result { Ok(()) => { self.clean = self.history.canvas().digest(); self.file = Some(path); self.message = "Saved".into(); }, Err(e) => self.message = e }
    }
    fn entries(path: &Path) -> Result<Vec<FileEntry>, String> {
        let mut result = Vec::new();
        for item in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            let metadata = item.metadata().map_err(|e| e.to_string())?;
            result.push(FileEntry { name: item.file_name().to_string_lossy().as_bytes().to_vec(), is_dir: metadata.is_dir(), size: metadata.len() });
        }
        Ok(result)
    }
    fn navigate(&mut self, path: PathBuf) {
        match Self::entries(&path) {
            Ok(entries) => { self.directory = path; let bytes = self.directory.to_string_lossy().as_bytes().to_vec(); if let Some(p) = &mut self.picker { p.navigate(bytes, entries); } },
            Err(e) => self.message = e,
        }
    }
    fn open_picker(&mut self) {
        let dir = self.file.as_ref().and_then(|p| p.parent()).filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("." )).to_path_buf();
        match Self::entries(&dir) {
            Ok(entries) => { self.directory = dir; self.picker = Some(FilePicker::new(Rect::new(150, 100, 500, 400), self.directory.to_string_lossy().as_bytes().to_vec(), entries, &Theme::dark())); },
            Err(e) => self.message = e,
        }
    }
    fn picker_result(&mut self, result: FilePickerResult) {
        match result {
            FilePickerResult::Cancelled => self.picker = None,
            FilePickerResult::NavigatePath(path) => self.navigate(PathBuf::from(String::from_utf8_lossy(&path).into_owned())),
            FilePickerResult::Selected(_) | FilePickerResult::NavigateDir(_) => {
                let name = self.picker.as_ref().and_then(|p| p.selected_entry()).map(|e| String::from_utf8_lossy(&e.name).into_owned());
                if let Some(name) = name { let path = self.directory.join(name); if matches!(result, FilePickerResult::NavigateDir(_)) { self.navigate(path); } else { self.picker = None; self.open_file(path); } }
            },
            FilePickerResult::Active => {},
        }
    }
    fn point(&self, x: i32, y: i32) -> Option<Point> {
        if x >= 0 && y >= TOP && (x as usize) < self.width && ((y - TOP) as usize) < self.height { Some((x, y - TOP)) } else { None }
    }
    fn draw_stroke(&self, image: &mut Canvas, points: &[Point]) {
        let Some(&last) = points.last() else { return };
        let color = self.color();
        let radius = match self.size { 0 => Radius::Small, 1 => Radius::Medium, _ => Radius::Large };
        match self.tool {
            0 => image.pencil(points, color), 1 => image.brush(points, radius, color),
            2 => image.line(points[0], last, color), 3 => image.rectangle(points[0], last, color),
            4 => image.circle(points[0], last, color), 5 => image.fill(points[0], color),
            _ => image.erase(points, radius),
        }
    }
    fn finish_stroke(&mut self) {
        let Some(points) = self.stroke.take() else { return };
        let image = self.history.canvas().preview(&points, |image, points| self.draw_stroke(image, points));
        if image.pixels() != self.history.canvas().pixels() {
            self.history.push(image);
            let color = self.color();
            if color != [255; 3] && !self.recent.contains(&color) { self.recent.copy_within(0..7, 1); self.recent[0] = color; }
        }
    }
    fn press(&mut self, x: i32, y: i32) {
        if y >= 23 && y < 39 && x >= 2 { let i = ((x - 2) / 38) as usize; if i < 7 && (x - 2) % 38 < 36 { self.tool = i; return; } }
        if y >= 41 && y < 57 && x >= 2 { let i = ((x - 2) / 22) as usize; if i < 3 && (x - 2) % 22 < 20 { self.size = i; return; } }
        if (58..76).contains(&y) && (0..780).contains(&x) { self.hue = node::hue_at(x as usize, 780); self.selected_color = None; return; }
        if (76..176).contains(&y) && (0..100).contains(&x) { (self.sat, self.val) = node::sv_at(x as usize, (y - 76) as usize, 100); self.selected_color = None; return; }
        if (90..108).contains(&y) && (108..252).contains(&x) { let i = ((x - 108) / 18) as usize; self.selected_color = Some(self.recent[i]); return; }
        if let Some(point) = self.point(x, y) { self.stroke = Some(vec![point]); }
    }
}
fn menu() -> MenuBar {
    let mut menu = MenuBar::new(Rect::new(0, 0, 800, MENU_BAR_HEIGHT), vec![
        Menu { label: b"File", items: vec![MenuItem::new(b"New", NEW, Shortcut::ctrl(b'N')), MenuItem::new(b"Open", OPEN, Shortcut::ctrl(b'O')), MenuItem::new(b"Save", SAVE, Shortcut::ctrl(b'S')), MenuItem::new(b"Quit", QUIT, Shortcut::ctrl(b'Q'))] },
        Menu { label: b"Edit", items: vec![MenuItem::disabled(b"Undo", UNDO, Shortcut::ctrl(b'Z')), MenuItem::disabled(b"Redo", REDO, Shortcut::ctrl_shift(b'Z'))] },
    ]);
    menu.layout(&Theme::dark()); menu
}
fn render(fb: &mut Surface, app: &mut App, menu: &MenuBar) {
    // Input acknowledgments wait for this frame. Retain the hue-dependent picker
    // on the GPU so unchanged input redraws only copy its pixels.
    if app.palette.as_ref().is_none_or(|(hue, _)| *hue != app.hue) {
        let mut palette = Surface::new(780, 118, libgfx::surface::Format::BGRX)
            .expect("Gus Kit picker surface");
        palette.batch(|fb| {
            fb.clear(Color::rgb(45, 45, 45));
            for x in 0..780 { let c = node::hsv_to_rgb(node::hue_at(x, 780), 255, 255); shapes::fill_rect(fb, x as i32, 0, 1, 18, Color::rgb(c[0], c[1], c[2])); }
            for y in 0..100 { for x in 0..100 { let (s,v) = node::sv_at(x,y,100); let c = node::hsv_to_rgb(app.hue,s,v); fb.put_pixel(x, y+18, Color::rgb(c[0],c[1],c[2])); } }
        });
        palette.wait().expect("Gus Kit picker rendering");
        app.palette = Some((app.hue, palette));
    }
    let preview = app.stroke.as_ref().map(|points| app.history.canvas().preview(points, |image, points| app.draw_stroke(image, points)));
    let canvas = preview.as_ref().unwrap_or_else(|| app.history.canvas());
    if let Some((previous, surface)) = &mut app.canvas_surface {
        let changed = surface.batch(|fb| canvas.render_changes(previous, |x, y, width, c|
            shapes::fill_rect(fb, x as i32, y as i32, width as i32, 1, Color::rgb(c[0], c[1], c[2]))));
        if changed {
            surface.wait().expect("Gus Kit canvas rendering");
            previous.clone_from(canvas);
        }
    } else {
        let mut surface = Surface::new(app.width, app.height, libgfx::surface::Format::BGRX)
            .expect("Gus Kit canvas surface");
        surface.batch(|fb| canvas.render_runs(|x, y, width, c|
            shapes::fill_rect(fb, x as i32, y as i32, width as i32, 1, Color::rgb(c[0], c[1], c[2]))));
        surface.wait().expect("Gus Kit canvas rendering");
        app.canvas_surface = Some((canvas.clone(), surface));
    }
    fb.batch(|fb| {
    fb.clear(Color::rgb(45, 45, 45));
    for (i, label) in LABELS.iter().enumerate() {
        let x = 2 + i * 38;
        shapes::fill_rect(fb, x as i32, 23, 36, 16, if i == app.tool { Color::rgb(100,140,220) } else { Color::rgb(60,60,60) });
        bitmap_font::draw_text(fb, label, x + 2, 25, Color::WHITE);
    }
    for (i, label) in [b"S", b"M", b"L"].iter().enumerate() {
        let x = 2 + i * 22;
        shapes::fill_rect(fb, x as i32, 41, 20, 16, if i == app.size { Color::rgb(100,140,220) } else { Color::rgb(60,60,60) });
        bitmap_font::draw_text(fb, *label, x + 3, 42, Color::WHITE);
    }
    libgfx::blit::blit(fb, &app.palette.as_ref().expect("picker rendered above").1, 0, 58);
    bitmap_font::draw_text(fb, b"Recent:", 108, 78, Color::WHITE);
    for (i, c) in app.recent.iter().enumerate() { shapes::fill_rect(fb, 108 + i as i32 * 18, 90, 16, 16, Color::rgb(c[0], c[1], c[2])); }
    libgfx::blit::blit(fb, &app.canvas_surface.as_ref().expect("canvas rendered above").1, 0, TOP);
    menu.draw(fb, &Theme::dark());
    if !app.message.is_empty() { bitmap_font::draw_text(fb, app.message.as_bytes(), 110, 150, Color::WHITE); }
    if let Some(picker) = &app.picker { picker.draw(fb, &Theme::dark()); }
    if app.confirm { shapes::fill_rect(fb, 180, 220, 440, 64, Color::rgb(30,30,30)); bitmap_font::draw_text(fb, b"Discard changes? D: discard  Esc: cancel", 190, 242, Color::WHITE); }
    });
}
fn publish(win: &mut Window, app: &App) {
    use breengel::tree::{Flag, Node, Role, Tree};
    let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
    let canvas_bounds = [0, TOP as i64, app.width as i64, app.height as i64];
    let mut children = vec![node::canvas(canvas_bounds, app.width, app.height, app.history.canvas().digest()), node::status(bounds, &node::Status { tool: NAMES[app.tool], size: ["small", "medium", "large"][app.size], color: app.color(), undo: app.history.undo_depth(), redo: app.history.redo_depth(), dirty: app.dirty(), file: app.file.as_ref().and_then(|p| p.to_str()), message: &app.message })];
    children.extend(node::tools(core::array::from_fn(|i| [(2 + i * 38) as i64, 23, 36, 16]), NAMES[app.tool]));
    children.push(node::hue([0,58,780,18])); children.push(node::sv([0,76,100,100]));
    if app.picker.is_some() { children.push(Node { key: "open".into(), parent: Some("root".into()), role: Role::Dialog, name: "Open".into(), value: serde_json::json!({"directory":app.directory.to_string_lossy()}), bounds: [150,100,500,400], clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] }); }
    if app.confirm { children.push(Node { key: "confirm".into(), parent: Some("root".into()), role: Role::Dialog, name: "Discard changes?".into(), value: serde_json::json!({"message":"D discards, Escape cancels"}), bounds: [180,220,440,64], clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] }); }
    win.set_tree(Tree { root: Node { key: "root".into(), parent: None, role: Role::Window, name: "Gus Kit".into(), value: serde_json::Value::Null, bounds, clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] }, children });
}
fn handle(app: &mut App, menu: &mut MenuBar, delivered: &breengel::Delivered) -> Handled {
    match delivered.event {
        Event::CloseRequested => app.request_quit(),
        Event::KeyPress { ascii, keycode, modifiers, .. } => {
            if matches!(keycode, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126) {
                return Handled::NoChange;
            }
            if app.picker.is_some() {
                use libbui::{Key, KeyInput};
                let key = match keycode { 1 => Key::Escape, 15 => Key::Tab, 28 => Key::Enter, 14 => Key::Backspace, 103 => Key::Up, 108 => Key::Down, 104 => Key::PageUp, 109 => Key::PageDown, 102 => Key::Home, 107 => Key::End, _ => Key::Char };
                let text = if ascii.is_ascii_graphic() || ascii == b' ' { (ascii as char).to_string() } else { String::new() };
                let input = KeyInput { key, shift: modifiers.shift, ctrl: modifiers.ctrl, alt: modifiers.alt, text: &text };
                let result = app.picker.as_mut().unwrap().key(&input); app.picker_result(result);
            } else if app.confirm { if keycode == 1 { app.confirm = false; } else if ascii.eq_ignore_ascii_case(&b'd') { app.quit = true; } }
            else if let Some(action) = menu.match_shortcut(if modifiers.ctrl && ascii == 0 { match keycode { 49 => b'N', 16 => b'Q', 44 => b'Z', 24 => b'O', 31 => b'S', _ => ascii } } else { ascii }, modifiers.ctrl, modifiers.shift, modifiers.alt) { app.action(action); }
            else if !modifiers.ctrl && !modifiers.alt { match ascii.to_ascii_lowercase() { b'p' => app.tool=0, b'b' => app.tool=1, b'l' => app.tool=2, b'r' => app.tool=3, b'c' => app.tool=4, b'f' => app.tool=5, b'e' => app.tool=6, b'1'..=b'3' => app.size=(ascii-b'1') as usize, _ => {} } }
        },
        Event::MouseButton { button: breengel::buttons::LEFT, pressed, x, y } if !app.confirm => {
            let input = InputState::from_raw(x,y, u32::from(pressed), u32::from(!pressed), delivered.time_ns / 1_000_000);
            if let Some(picker) = &mut app.picker { let result = picker.update(&input); app.picker_result(result); return Handled::Redraw; }
            let was_open = menu.is_open();
            if let MenuEvent::Activated(action) = menu.update(&input, &Theme::dark()) { app.action(action); }
            if !was_open && !menu.is_open() { if pressed { app.press(x,y); } else { if let Some(point) = app.point(x,y) { if let Some(stroke) = &mut app.stroke { stroke.push(point); } } app.finish_stroke(); } }
        },
        Event::MouseMove { x,y } if !app.confirm => {
            if let Some(picker) = &mut app.picker { let input = InputState::from_raw(x,y,0,0,delivered.time_ns / 1_000_000); let result = picker.update(&input); app.picker_result(result); }
            else if menu.is_open() { let input = InputState::from_raw(x, y, 0, 0, delivered.time_ns / 1_000_000); menu.update(&input, &Theme::dark()); }
            else if let (Some(point), Some(stroke)) = (app.point(x,y), &mut app.stroke) { stroke.push(point); }
            else { return Handled::NoChange; }
        },
        Event::PointerCancel => { if app.stroke.take().is_none() { return Handled::NoChange; } },
        _ => return Handled::NoChange,
    }
    menu.set_enabled(UNDO, app.history.undo_depth() > 0);
    menu.set_enabled(REDO, app.history.redo_depth() > 0);
    Handled::Redraw
}
fn main() {
    let mut args = std::env::args().skip(1);
    let mut next = args.next();
    let (width,height) = if next.as_deref() == Some("--canvas") {
        let value = args.next().unwrap_or_default();
        let parsed = value.split_once('x').and_then(|(w,h)| Some((w.parse::<usize>().ok()?, h.parse::<usize>().ok()?)));
        next = args.next();
        match parsed { Some((w,h)) if w > 0 && h > 0 && w <= 800 && h <= 600 - TOP as usize => (w,h), _ => { eprintln!("invalid --canvas size"); std::process::exit(2); } }
    } else { (800, 600 - TOP as usize) };
    if args.next().is_some() { eprintln!("usage: guskit [--canvas WxH] [FILE]"); std::process::exit(2); }
    let image = Canvas::new(width,height); let clean = image.digest();
    let mut app = App { width,height,history: History::new(image),clean,tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0,0,0],[255,0,0],[0,255,0],[0,0,255],[255,255,0],[255,0,255],[0,255,255],[128,128,128]],confirm:false,quit:false,file:next.map(PathBuf::from),message:String::new(),picker:None,directory:PathBuf::from("."),palette:None,canvas_surface:None };
    if let Some(path) = app.file.clone() { if path.exists() { app.open_file(path); } }
    let mut win = Window::new(b"Gus Kit", 800, 600).unwrap_or_else(|e| { eprintln!("Gus Kit: {e}"); std::process::exit(1) });
    let mut menu = menu();
    let mut redraw = true;
    loop {
        menu.set_enabled(UNDO, app.history.undo_depth() > 0); menu.set_enabled(REDO, app.history.redo_depth() > 0);
        if redraw {
            if let Ok(surface) = win.surface() { render(surface, &mut app, &menu); }
            publish(&mut win, &app); let _ = win.present();
        }
        if app.quit { break; }
        redraw = false;
        let mut pointer_motion = false;
        let count = win.wait_dispatch(100_000_000, |delivered| {
            pointer_motion |= matches!(delivered.event, Event::MouseMove { .. });
            let handled = handle(&mut app, &mut menu, delivered);
            redraw |= handled == Handled::Redraw;
            handled
        }).unwrap_or_else(|e| { eprintln!("Gus Kit dispatch: {e}"); std::process::exit(1) });
        // MAP_TREE is passive. Its attach probe needs a freshly sealed tree
        // page, which is published with a pixel commit, even on idle motion.
        redraw |= pointer_motion && win.tree_reader_count() != 0;
        if count != 0 && !redraw { let _ = win.commit_no_change(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use breengel::{Delivered, Modifiers};

    #[test]
    fn passive_events_acknowledge_without_repainting_and_strokes_still_preview() {
        let image = Canvas::new(200, 120);
        let mut app = App { width:200,height:120,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),palette:None,canvas_surface:None };
        let mut menu = menu();
        let mut event = Delivered { serial:1,time_ns:1,clicks:0,
            event:Event::KeyRelease { keycode:25,modifiers:Modifiers::default() } };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::NoChange);
        event.event = Event::Text { text:"p".into() };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::NoChange);
        event.event = Event::MouseMove { x:20,y:186 };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::NoChange);
        event.event = Event::MouseButton { button:breengel::buttons::LEFT,pressed:true,x:20,y:186 };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::Redraw);
        event.event = Event::MouseMove { x:40,y:186 };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::Redraw);
        assert_eq!(app.history.undo_depth(), 0, "preview must not commit the stroke");
        event.event = Event::MouseButton { button:breengel::buttons::LEFT,pressed:false,x:40,y:186 };
        assert_eq!(handle(&mut app, &mut menu, &event), Handled::Redraw);
        assert_eq!(app.history.undo_depth(), 1);
    }
}
