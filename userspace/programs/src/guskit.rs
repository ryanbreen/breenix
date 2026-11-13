//! Gus Kit drawing window.
use breengel::{Color, Event, Handled, Surface, Window};
use libbui::{InputState, Rect, Theme, shortcut::Shortcut, widget::menu_bar::{Menu, MenuBar, MenuEvent, MenuItem, MENU_BAR_HEIGHT}};
use libgfx::{bitmap_font, shapes, ttf_font::{self, CachedFont}, surface::Rect as Clip};
use libbui::widget::file_picker::{FileEntry, FilePicker, FilePickerResult};
use std::path::{Path, PathBuf};

#[path = "guskit/canvas.rs"] mod canvas;
#[path = "guskit/collab.rs"] mod collab;
use libcollab::wire::DrawOp;
#[path = "guskit/node.rs"] mod node;
#[path = "guskit/session_dialog.rs"] mod session_dialog;
#[path = "guskit/files.rs"] mod files;
#[path = "guskit/save_barrier.rs"] mod save_barrier;
use canvas::{Canvas, History, Point, Radius};

fn session_title(name: &str) -> String {
    let mut end = name.len().min(libbreenix::window::abi::limits::MAX_TITLE - "Gus Kit ()".len());
    while !name.is_char_boundary(end) { end -= 1; }
    format!("Gus Kit ({})", &name[..end])
}

#[path = "guskit/layout.rs"] mod layout;
use layout::Layout;
const NAMES: [&str; 7] = ["pencil", "brush", "line", "rectangle", "circle", "fill", "eraser"];
const LABELS: [&[u8]; 7] = [b"Pen", b"Brsh", b"Line", b"Rect", b"Circ", b"Fill", b"Ers"];
const NEW: u16 = 1;
const QUIT: u16 = 2;
const UNDO: u16 = 3;
const REDO: u16 = 4;
const OPEN: u16 = 5;
const SAVE: u16 = 6;
const SAVE_AS: u16 = 7;
const HOST_SESSION: u16 = 8;
const JOIN_SESSION: u16 = 9;

struct SessionDialog {
    model: session_dialog::Dialog, mode: session_dialog::Mode,
    address: libbui::widget::text_field::TextField,
    name: libbui::widget::text_field::TextField,
}
impl SessionDialog {
    fn new(mode: session_dialog::Mode, layout: &Layout) -> Self {
        let mut address = libbui::widget::text_field::TextField::new(layout.logical(layout.address), 64);
        let mut name = libbui::widget::text_field::TextField::new(layout.logical(layout.name), 64);
        address.set_focused(mode == session_dialog::Mode::Join);
        name.set_focused(mode == session_dialog::Mode::Host);
        Self { model: session_dialog::Dialog::new(mode), mode, address, name }
    }
    fn key(&mut self, input: &libbui::KeyInput<'_>) -> Option<session_dialog::Outcome> {
        match input.key {
            libbui::Key::Escape => Some(self.model.cancel()),
            libbui::Key::Enter => Some(self.model.submit(self.address.text(), self.name.text())),
            libbui::Key::Tab if self.mode == session_dialog::Mode::Join => {
                let address = !self.address.is_focused();
                self.address.set_focused(address); self.name.set_focused(!address); None
            }
            _ => { if self.address.is_focused() { self.address.key(input); } else { self.name.key(input); } None }
        }
    }
    fn update(&mut self, input: &InputState) {
        if self.mode == session_dialog::Mode::Join { self.address.update(input); }
        self.name.update(input);
    }
}
struct App {
    layout: Layout,
    damage: Option<Clip>,
    width: usize, height: usize, history: History, clean: [u8; 32],
    tool: usize, size: usize, hue: u16, sat: u8, val: u8,
    stroke: Option<Vec<Point>>, selected_color: Option<[u8; 3]>, recent: [[u8; 3]; 8], confirm: bool, quit: bool,
    file: Option<PathBuf>, message: String, picker: Option<FilePicker>, directory: PathBuf, save_as: bool,
    palette: Option<(u16, Surface)>,
    canvas_surface: Option<(Canvas, Surface)>,
    session: Option<collab::Session>,
    session_dialog: Option<SessionDialog>,
}
impl App {
    fn resize_view(&mut self, width: u32, height: u32) {
        if self.session.is_some() { return; }
        self.width = self.width.max(width as usize);
        self.height = self.height.max((height as usize).saturating_sub(self.layout.canvas_y as usize));
        if let Some(clean) = self.history.clean_after_grow(self.width, self.height, self.clean) { self.clean = clean; }
        self.history.grow(self.width, self.height);
    }
    fn color(&self) -> [u8; 3] { self.selected_color.unwrap_or_else(|| node::hsv_to_rgb(self.hue, self.sat, self.val)) }
    fn canvas(&self) -> &Canvas { self.session.as_ref().map_or_else(|| self.history.canvas(), |s| &s.core.model.canvas) }
    fn undo_depth(&self) -> usize { self.session.as_ref().map_or_else(||self.history.undo_depth(),|s|s.core.model.undo_depth()) }
    fn redo_depth(&self) -> usize { if self.session.is_some() {0} else {self.history.redo_depth()} }
    fn status(&self) -> &str { if !self.message.is_empty() { &self.message } else { self.session.as_ref().map_or("",|s|s.core.model.status.as_str()) } }
    // Keep the ended session’s drawing and status available until another starts.
    fn can_start_session(&self) -> bool {
        self.session.as_ref().is_none_or(|session| session.core.model.state == "disconnected")
    }
    fn dirty(&self) -> bool { self.canvas().digest() != self.clean }
    fn request_quit(&mut self) { if self.dirty() { self.confirm = true; } else { self.quit = true; } }
    fn action(&mut self, action: u16) {
        match action {
            HOST_SESSION | JOIN_SESSION if self.can_start_session() => { self.session_dialog = Some(SessionDialog::new(if action == HOST_SESSION { session_dialog::Mode::Host } else { session_dialog::Mode::Join }, &self.layout)); },
            OPEN => { if self.session.is_none() { self.open_picker(false); } else { self.message="Open an image before starting a session".into(); } },
            SAVE_AS => self.open_picker(true),
            SAVE => self.save(),
            NEW => { if let Some(session)=&mut self.session { session.core.submit(DrawOp::Clear); } else { self.history.push(Canvas::new(self.width, self.height)); } self.file = None; self.stroke = None; self.message.clear(); },
            QUIT => self.request_quit(),
            UNDO => { if let Some(session)=&mut self.session { session.core.submit(DrawOp::Undo); } else { self.history.undo(); } },
            REDO => { if self.session.is_none() { self.history.redo(); } },
            _ => {}
        }
    }
    fn dialog_result(&mut self, result: Option<session_dialog::Outcome>) {
        match result {
            Some(session_dialog::Outcome::Cancelled) => self.session_dialog = None,
            Some(session_dialog::Outcome::Start(host, address, port, name)) => {
                let result = if host { collab::Session::host(address, port, &name, self.canvas().clone(), self.width, self.height) } else { collab::Session::join(address, port, &name) };
                match result { Ok(session) => { self.session = Some(session); self.session_dialog = None; self.message.clear(); }, Err(e) => self.message = format!("Session: {e}") }
            }
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
    fn save_to(&mut self, path: PathBuf) {
        let result = files::encode(&path.to_string_lossy(), self.canvas(), self.width, self.height)
            .and_then(|bytes| match save_barrier::save(&path, &bytes) {
                libbreenix::atomic_file::SaveResult::Saved => Ok(()),
                libbreenix::atomic_file::SaveResult::Failed(e) | libbreenix::atomic_file::SaveResult::Unconfirmed(e) => Err(e.to_string()),
            });
        match result { Ok(()) => { self.clean = self.canvas().digest(); self.file = Some(path); self.message = "Saved".into(); }, Err(e) => self.message = e }
    }
    fn save(&mut self) {
        let mut path = self.file.clone().unwrap_or_default();
        let result = if self.file.is_some() {
            files::encode(&path.to_string_lossy(), self.canvas(), self.width, self.height)
                .and_then(|bytes| match save_barrier::save(&path, &bytes) {
                    libbreenix::atomic_file::SaveResult::Saved => Ok(()),
                    libbreenix::atomic_file::SaveResult::Failed(e) | libbreenix::atomic_file::SaveResult::Unconfirmed(e) => Err(e.to_string()),
                })
        } else {
            // Reserve the first free name atomically; another drawing cannot be
            // replaced even if it appears between choosing a name and saving.
            files::encode("untitled.png", self.canvas(), self.width, self.height).and_then(|bytes| {
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
        match result { Ok(()) => { self.clean = self.canvas().digest(); self.file = Some(path); self.message = "Saved".into(); }, Err(e) => self.message = e }
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
    fn open_picker(&mut self, save_as: bool) {
        let dir = self.file.as_ref().and_then(|p| p.parent()).filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("." )).to_path_buf();
        match Self::entries(&dir) {
            Ok(entries) => { self.save_as = save_as; self.directory = dir; self.picker = Some(FilePicker::new(self.layout.logical(self.layout.picker), self.directory.to_string_lossy().as_bytes().to_vec(), entries, &Theme::dark())); },
            Err(e) => self.message = e,
        }
    }
    fn picker_result(&mut self, result: FilePickerResult) {
        match result {
            FilePickerResult::Cancelled => self.picker = None,
            FilePickerResult::NavigatePath(path) => {
                let path = PathBuf::from(String::from_utf8_lossy(&path).into_owned());
                if self.save_as && !path.is_dir() { self.picker = None; self.save_to(path); }
                else { self.navigate(path); }
            },
            FilePickerResult::Selected(_) | FilePickerResult::NavigateDir(_) => {
                let name = self.picker.as_ref().and_then(|p| p.selected_entry()).map(|e| String::from_utf8_lossy(&e.name).into_owned());
                if let Some(name) = name { let path = self.directory.join(name); if matches!(result, FilePickerResult::NavigateDir(_)) { self.navigate(path); } else { self.picker = None; if self.save_as { self.save_to(path); } else { self.open_file(path); } } }
            },
            FilePickerResult::Active => {},
        }
    }
    fn point(&self, x: i32, y: i32) -> Option<Point> {
        self.layout.point(x, y, self.width, self.height)
    }
    fn stroke_bounds(&self) -> Option<Clip> {
        let points = self.stroke.as_ref()?;
        let &(x, y) = points.first()?;
        // Fill can reach the whole connected component. Circle is centred on
        // the press; the L1 radius conservatively bounds its Euclidean radius.
        if self.tool == 5 { return Some(Clip::new(0, 0, self.width as i32, self.height as i32)); }
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
        for &(x, y) in points { x0=x0.min(x); y0=y0.min(y); x1=x1.max(x); y1=y1.max(y); }
        if self.tool == 4 {
            let &(last_x, last_y) = points.last()?;
            let radius = (last_x-x).abs() + (last_y-y).abs();
            (x0,y0,x1,y1)=(x-radius,y-radius,x+radius,y+radius);
        }
        let radius = if matches!(self.tool, 1 | 6) { [2,5,10][self.size] } else { 0 };
        x0=(x0-radius).max(0); y0=(y0-radius).max(0);
        x1=(x1+radius+1).min(self.width as i32); y1=(y1+radius+1).min(self.height as i32);
        (x1>x0 && y1>y0).then_some(Clip::new(x0,y0,x1-x0,y1-y0))
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
        let ops=collab::stroke(self.tool,self.size,self.color(),&points);
        if let Some(session)=&mut self.session {
            session.core.submit_stroke(ops);
            return;
        }
        let image = self.canvas().preview(&points, |image, points| self.draw_stroke(image, points));
        if image.pixels() != self.canvas().pixels() {
            self.history.push(image);
            let color = self.color();
            if color != [255; 3] && !self.recent.contains(&color) { self.recent.copy_within(0..7, 1); self.recent[0] = color; }
        }
    }
    fn press(&mut self, x: i32, y: i32) {
        let l = &self.layout;
        if let Some(i) = l.tools.iter().position(|r| r.contains(x, y)) { self.tool = i; return; }
        if let Some(i) = l.sizes.iter().position(|r| r.contains(x, y)) { self.size = i; return; }
        if l.hue.contains(x, y) { self.hue = l.hue_at(x); self.selected_color = None; return; }
        if l.sv.contains(x, y) { (self.sat, self.val) = l.sv_at(x, y); self.selected_color = None; return; }
        if let Some(i) = l.recent.iter().position(|r| r.contains(x, y)) { self.selected_color = Some(self.recent[i]); return; }
        if self.session.as_ref().is_none_or(|s|s.core.model.state == "connected") { if let Some(point) = self.point(x, y) { self.stroke = Some(vec![point]); } }
    }
}
fn menu() -> MenuBar {
    let mut menu = MenuBar::new(Rect::new(0, 0, 800, MENU_BAR_HEIGHT), vec![
        Menu { label: b"File", items: vec![MenuItem::new(b"New", NEW, Shortcut::ctrl(b'N')), MenuItem::new(b"Open", OPEN, Shortcut::ctrl(b'O')), MenuItem::new(b"Save", SAVE, Shortcut::ctrl(b'S')), MenuItem::new(b"Save As", SAVE_AS, Shortcut::ctrl_shift(b'S')), MenuItem::new(b"Quit", QUIT, Shortcut::ctrl(b'Q'))] },
        Menu { label: b"Session", items: vec![MenuItem::new(b"Host Session...", HOST_SESSION, Shortcut::NONE), MenuItem::new(b"Join Session...", JOIN_SESSION, Shortcut::NONE)] },
        Menu { label: b"Edit", items: vec![MenuItem::disabled(b"Undo", UNDO, Shortcut::ctrl(b'Z')), MenuItem::disabled(b"Redo", REDO, Shortcut::ctrl_shift(b'Z'))] },
    ]);
    menu.layout(&Theme::dark()); menu
}
fn draw_label(fb: &mut Surface, font: Option<&mut CachedFont>, size: f32, text: &str, x: i32, y: i32, clip: Clip) {
    let (size, raster) = libbui::text::scaled_font(size, fb.ui_scale());
    fb.push_clip(clip);
    if let Some(font) = font {
        ttf_font::draw_text(fb, font, text, x, y, size, Color::WHITE);
    } else {
        bitmap_font::draw_text_at(fb, text.as_bytes(), x as usize, y as usize, Color::WHITE, raster);
    }
    fb.pop_clip();
}
fn union(a: Clip, b: Clip) -> Clip {
    let x = a.x.min(b.x); let y = a.y.min(b.y);
    Clip::new(x,y,(a.x+a.w).max(b.x+b.w)-x,(a.y+a.h).max(b.y+b.h)-y)
}
fn render(fb: &mut Surface, app: &mut App, menu: &MenuBar, font: &mut Option<CachedFont>, font_size: f32) {
    let l = app.layout;
    // Input acknowledgments wait for this frame. Retain the hue-dependent picker
    // on the GPU so unchanged input redraws only copy its pixels.
    if app.palette.as_ref().is_none_or(|(hue, surface)| *hue != app.hue || surface.width() != l.palette.w as usize || surface.height() != l.palette.h as usize) {
        let mut palette = Surface::new(l.palette.w as usize, l.palette.h as usize, libgfx::surface::Format::BGRX)
            .expect("Gus Kit picker surface");
        palette.batch(|fb| {
            fb.clear(Color::rgb(45, 45, 45));
            for x in 0..l.hue.w { let c = node::hsv_to_rgb(l.hue_at(l.hue.x + x), 255, 255); shapes::fill_rect(fb, x, l.hue.y - l.palette.y, 1, l.hue.h, Color::rgb(c[0], c[1], c[2])); }
            for y in 0..l.sv.h { for x in 0..l.sv.w { let (s,v) = l.sv_at(l.sv.x + x, l.sv.y + y); let c = node::hsv_to_rgb(app.hue,s,v); fb.put_pixel(x as usize, (y + l.sv.y - l.palette.y) as usize, Color::rgb(c[0],c[1],c[2])); } }
        });
        palette.wait().expect("Gus Kit picker rendering");
        app.palette = Some((app.hue, palette));
    }
    let mut cached = app.canvas_surface.take();
    let canvas = app.canvas();
    if cached.as_ref().is_some_and(|(_,surface)|surface.width()!=app.width || surface.height()!=app.height) { cached=None; }
    if let Some((previous, surface)) = &mut cached {
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
        cached = Some((canvas.clone(), surface));
    }
    app.canvas_surface=cached;
    if let Some(damage) = app.damage { fb.push_clip(damage); }
    fb.batch(|fb| {
    fb.clear(Color::rgb(45, 45, 45));
    for (i, label) in LABELS.iter().enumerate() {
        let r = l.tools[i];
        shapes::fill_rect(fb, r.x, r.y, r.w, r.h, if i == app.tool { Color::rgb(100,140,220) } else { Color::rgb(60,60,60) });
        draw_label(fb, font.as_mut(), font_size, core::str::from_utf8(label).unwrap(), l.tool_text[i].0, l.tool_text[i].1, Clip::new(r.x,r.y,r.w,r.h));
    }
    for (i, label) in [b"S", b"M", b"L"].iter().enumerate() {
        let r = l.sizes[i];
        shapes::fill_rect(fb, r.x, r.y, r.w, r.h, if i == app.size { Color::rgb(100,140,220) } else { Color::rgb(60,60,60) });
        draw_label(fb, font.as_mut(), font_size, core::str::from_utf8(*label).unwrap(), l.size_text[i].0, l.size_text[i].1, Clip::new(r.x,r.y,r.w,r.h));
    }
    libgfx::blit::blit(fb, &app.palette.as_ref().expect("picker rendered above").1, l.palette.x, l.palette.y);
    let r = l.recent_label;
    draw_label(fb, font.as_mut(), font_size, "Recent:", l.recent_text.0, l.recent_text.1, Clip::new(r.x,r.y,r.w,r.h));
    for (r, c) in l.recent.iter().zip(&app.recent) { shapes::fill_rect(fb, r.x, r.y, r.w, r.h, Color::rgb(c[0], c[1], c[2])); }
    let r = l.canvas(app.width, app.height);
    libgfx::blit::blit(fb, &app.canvas_surface.as_ref().expect("canvas rendered above").1, r.x, r.y);
    if let Some(bounds) = app.stroke_bounds() {
        let mut patch = app.canvas().region(bounds.x as usize, bounds.y as usize, bounds.w as usize, bounds.h as usize);
        let points: Vec<_> = app.stroke.as_ref().expect("preview bounds require a stroke").iter()
            .map(|&(x,y)| (x-bounds.x,y-bounds.y)).collect();
        app.draw_stroke(&mut patch, &points);
        patch.render_runs(|x,y,width,c| shapes::fill_rect(fb,
            r.x+bounds.x+x as i32,r.y+bounds.y+y as i32,width as i32,1,Color::rgb(c[0],c[1],c[2])));
    }
    menu.draw(fb, &Theme::dark());
    let r = l.status;
    if !app.status().is_empty() { draw_label(fb, font.as_mut(), font_size, app.status(), r.x, r.y, Clip::new(r.x,r.y,r.w,r.h)); }
    if let Some(picker) = &app.picker { picker.draw(fb, &Theme::dark()); }
    if let Some(dialog) = &app.session_dialog {
        let r = l.session;
        shapes::fill_rect(fb, r.x, r.y, r.w, r.h, Theme::dark().panel_bg);
        let title = if dialog.mode == session_dialog::Mode::Host { "Host Session" } else { "Join Session" };
        let r = l.session_title;
        draw_label(fb, font.as_mut(), font_size, title, r.x, r.y, Clip::new(r.x,r.y,r.w,r.h));
        if dialog.mode == session_dialog::Mode::Join {
            let r = l.address_label;
            draw_label(fb, font.as_mut(), font_size, "Address", r.x, r.y, Clip::new(r.x,r.y,r.w,r.h));
            dialog.address.draw(fb, &Theme::dark());
        }
        let r = l.name_label;
        draw_label(fb, font.as_mut(), font_size, "Name", r.x, r.y, Clip::new(r.x,r.y,r.w,r.h));
        dialog.name.draw(fb, &Theme::dark());
        let status = if dialog.model.status().is_empty() { app.message.as_str() } else { dialog.model.status() };
        let r = l.session_status;
        draw_label(fb, font.as_mut(), font_size, status, r.x, r.y, Clip::new(r.x,r.y,r.w,r.h));
    }
    if app.confirm {
        let r = l.confirm;
        shapes::fill_rect(fb, r.x, r.y, r.w, r.h, Color::rgb(30,30,30));
        let r = l.confirm_label;
        draw_label(fb, font.as_mut(), font_size, "Discard changes? D: discard  Esc: cancel", r.x, r.y, Clip::new(r.x,r.y,r.w,r.h));
    }
    });
    if app.damage.is_some() { fb.pop_clip(); }
}
fn publish(win: &mut Window, app: &App, menu: &MenuBar, font_size: f32) {
    use libbui::widget::Widget;
    use breengel::tree::{Flag, Node, Role, Tree};
    let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
    let l = app.layout;
    let canvas_bounds = l.canvas(app.width, app.height).as_tree_rect();
    let mut children = vec![node::canvas(canvas_bounds, app.width, app.height, app.canvas().digest()), node::status(l.status.as_tree_rect(), &node::Status { tool: NAMES[app.tool], size: ["small", "medium", "large"][app.size], color: app.color(), undo: app.undo_depth(), redo: app.redo_depth(), dirty: app.dirty(), file: app.file.as_ref().and_then(|p| p.to_str()), message: app.status(), font_path: win.display_path(), font_size })];
    if let Some(session)=&app.session { children.push(node::collab(bounds,&session.core.model)); }
    children.extend(menu.tree_nodes("menu", Some("root"), Rect::new(0, 0, win.width() as i32 / l.scale, win.height() as i32 / l.scale), &Theme::dark()).into_iter().map(|mut n| { n.bounds = n.bounds.map(|v| v * i64::from(l.scale)); n.clip = n.clip.map(|v| v * i64::from(l.scale)); n }));
    children.extend(node::tools(l.tools.map(|r| r.as_tree_rect()), NAMES[app.tool]));
    children.push(node::hue(l.hue.as_tree_rect())); children.push(node::sv(l.sv.as_tree_rect()));
    if app.picker.is_some() { children.push(Node { key: if app.save_as { "save-as" } else { "open" }.into(), parent: Some("root".into()), role: Role::Dialog, name: if app.save_as { "Save As" } else { "Open" }.into(), value: serde_json::json!({"directory":app.directory.to_string_lossy()}), bounds: l.picker.as_tree_rect(), clip: l.picker.as_tree_rect(), flags: vec![Flag::Enabled, Flag::Visible] }); }
    if let Some(dialog) = &app.session_dialog { children.extend(node::session_dialog(dialog.mode == session_dialog::Mode::Host, dialog.address.text(), dialog.name.text(), if dialog.model.status().is_empty() { &app.message } else { dialog.model.status() }, dialog.address.is_focused(), dialog.name.is_focused(), [l.session.as_tree_rect(), l.address.as_tree_rect(), l.name.as_tree_rect()])); }
    if app.confirm { children.push(Node { key: "confirm".into(), parent: Some("root".into()), role: Role::Dialog, name: "Discard changes?".into(), value: serde_json::json!({"message":"D discards, Escape cancels"}), bounds: l.confirm.as_tree_rect(), clip: l.confirm.as_tree_rect(), flags: vec![Flag::Enabled, Flag::Visible] }); }
    win.set_tree(Tree { root: Node { key: "root".into(), parent: None, role: Role::Window, name: app.session.as_ref().map_or_else(||"Gus Kit".into(),|s|format!("Gus Kit ({})",s.core.model.name)), value: serde_json::Value::Null, bounds, clip: bounds, flags: vec![Flag::Enabled, Flag::Visible] }, children });
}
fn handle(app: &mut App, menu: &mut MenuBar, delivered: &breengel::Delivered) -> Handled {
    match delivered.event {
        Event::CloseRequested => app.request_quit(),
        Event::KeyPress { ascii, keycode, modifiers, .. } => {
            if matches!(keycode, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126) {
                return Handled::NoChange;
            }
            if app.session_dialog.is_some() {
                let key = match keycode { 1 => libbui::Key::Escape, 15 => libbui::Key::Tab, 28 => libbui::Key::Enter, 14 => libbui::Key::Backspace, 111 => libbui::Key::Delete, 105 => libbui::Key::Left, 106 => libbui::Key::Right, 102 => libbui::Key::Home, 107 => libbui::Key::End, _ => libbui::Key::Char };
                // Printable input arrives separately as Event::Text.
                let input = libbui::KeyInput { key, shift: modifiers.shift, ctrl: modifiers.ctrl, alt: modifiers.alt, text: "" };
                let result = app.session_dialog.as_mut().unwrap().key(&input); app.dialog_result(result);
            } else if app.picker.is_some() {
                use libbui::{Key, KeyInput};
                let key = match keycode { 1 => Key::Escape, 15 => Key::Tab, 28 => Key::Enter, 14 => Key::Backspace, 103 => Key::Up, 108 => Key::Down, 104 => Key::PageUp, 109 => Key::PageDown, 102 => Key::Home, 107 => Key::End, _ => Key::Char };
                let text = if ascii.is_ascii_graphic() || ascii == b' ' { (ascii as char).to_string() } else { String::new() };
                let input = KeyInput { key, shift: modifiers.shift, ctrl: modifiers.ctrl, alt: modifiers.alt, text: &text };
                let result = app.picker.as_mut().unwrap().key(&input); app.picker_result(result);
            } else if app.confirm { if keycode == 1 { app.confirm = false; } else if ascii.eq_ignore_ascii_case(&b'd') { app.quit = true; } }
            else if let Some(action) = menu.match_shortcut(if modifiers.ctrl && ascii == 0 { match keycode { 49 => b'N', 16 => b'Q', 44 => b'Z', 24 => b'O', 31 => b'S', _ => ascii } } else { ascii }, modifiers.ctrl, modifiers.shift, modifiers.alt) { app.action(action); }
            else if !modifiers.ctrl && !modifiers.alt { match ascii.to_ascii_lowercase() { b'p' => app.tool=0, b'b' => app.tool=1, b'l' => app.tool=2, b'r' => app.tool=3, b'c' => app.tool=4, b'f' => app.tool=5, b'e' => app.tool=6, b'1'..=b'3' => app.size=(ascii-b'1') as usize, _ => {} } }
        },
        Event::Text { ref text } if app.session_dialog.is_some() => {
            // bwm also emits control text after Tab/Enter key actions.
            if text.chars().all(char::is_control) { return Handled::NoChange; }
            let input = libbui::KeyInput { key: libbui::Key::Char, shift: false, ctrl: false, alt: false, text };
            let result = app.session_dialog.as_mut().unwrap().key(&input); app.dialog_result(result);
        },
        Event::Text { ref text } if app.picker.is_some() => {
            let input = libbui::KeyInput { key: libbui::Key::Char, shift: false, ctrl: false, alt: false, text };
            let result = app.picker.as_mut().unwrap().key(&input);
            app.picker_result(result);
        },
        Event::MouseButton { button: breengel::buttons::LEFT, pressed, x, y } if !app.confirm => {
            let input = InputState::from_raw(x / app.layout.scale,y / app.layout.scale, u32::from(pressed), u32::from(!pressed), delivered.time_ns / 1_000_000);
            if let Some(dialog) = &mut app.session_dialog { dialog.update(&input); return Handled::Redraw; }
            if let Some(picker) = &mut app.picker { let result = picker.update(&input); app.picker_result(result); return Handled::Redraw; }
            let was_open = menu.is_open();
            if let MenuEvent::Activated(action) = menu.update(&input, &Theme::dark()) { app.action(action); }
            if !was_open && !menu.is_open() { if pressed { app.press(x,y); } else { if let Some(point) = app.point(x,y) { if let Some(stroke) = &mut app.stroke { stroke.push(point); } } app.finish_stroke(); } }
        },
        Event::MouseMove { x,y } if !app.confirm => {
            if app.session_dialog.is_some() { return Handled::NoChange; }
            if let Some(picker) = &mut app.picker { let input = InputState::from_raw(x / app.layout.scale,y / app.layout.scale,0,0,delivered.time_ns / 1_000_000); let result = picker.update(&input); app.picker_result(result); }
            else if menu.is_open() { let input = InputState::from_raw(x / app.layout.scale, y / app.layout.scale, 0, 0, delivered.time_ns / 1_000_000); menu.update(&input, &Theme::dark()); }
            else if let (Some(point), Some(stroke)) = (app.point(x,y), &mut app.stroke) { stroke.push(point); }
            else { return Handled::NoChange; }
        },
        Event::PointerCancel => { if app.stroke.take().is_none() { return Handled::NoChange; } },
        _ => return Handled::NoChange,
    }
    menu.set_enabled(HOST_SESSION, app.can_start_session()); menu.set_enabled(JOIN_SESSION, app.can_start_session());
    menu.set_enabled(UNDO, app.session.is_some() || app.undo_depth() > 0);
    menu.set_enabled(REDO, app.redo_depth() > 0);
    Handled::Redraw
}
fn dispatch_input(app: &mut App, menu: &mut MenuBar, delivered: &breengel::Delivered, redraw: &mut bool) -> Handled {
    let before = if matches!(delivered.event, Event::MouseMove { .. })
        && !menu.is_open() && app.picker.is_none() && app.session_dialog.is_none() && !app.confirm {
        app.stroke_bounds()
    } else { None };
    let handled = handle(app, menu, delivered);
    if handled == Handled::Redraw {
        let delta = before.zip(app.stroke_bounds()).map(|(a,b)| {
            let r = union(a,b); Clip::new(r.x,r.y+app.layout.canvas_y,r.w,r.h)
        });
        app.damage = if *redraw { app.damage.zip(delta).map(|(a,b)| union(a,b)) } else { delta };
        *redraw = true;
    }
    handled
}
#[path = "guskit/options.rs"] mod options;
use options::Options;
fn main() {
    let options=Options::parse(std::env::args().skip(1)).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(2) });
    let (width,height)=(options.width,options.height);
    let next=options.file;
    let image = Canvas::new(width,height); let clean = image.digest();
    let mut app = App { damage: None, layout: Layout::new(1), width,height,history: History::new(image),clean,tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0,0,0],[255,0,0],[0,255,0],[0,0,255],[255,255,0],[255,0,255],[0,255,255],[128,128,128]],confirm:false,quit:false,file:next.map(PathBuf::from),message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
    if let Some(path) = app.file.clone() { if path.exists() { app.open_file(path); } }
    if let Some((host,address,port,name))=options.session {
        let result=if host {collab::Session::host(address,port,&name,app.canvas().clone(),width,height)} else {collab::Session::join(address,port,&name)};
        app.session=Some(result.unwrap_or_else(|e| {eprintln!("Gus Kit session: {e}");std::process::exit(1)}));
    }
    let mut title=app.session.as_ref().map_or_else(||"Gus Kit".into(),|s|session_title(&s.core.model.name));
    let mut win = Window::new(title.as_bytes(), 800, 600).unwrap_or_else(|e| { eprintln!("Gus Kit: {e}"); std::process::exit(1) });
    let scale = win.surface().expect("Gus Kit surface").ui_scale();
    app.layout = Layout::new(scale);
    if scale != 1 { win.apply_resize(800 * scale, 600 * scale).expect("scaled Gus Kit window"); }
    let mut menu = menu();
    let mut font = win.take_display_font();
    let mut font_size = win.display_size();
    let mut redraw = true;
    let mut initial_resize = (scale != 1).then_some((800 * scale, 600 * scale));
    loop {
        menu.set_enabled(HOST_SESSION, app.can_start_session()); menu.set_enabled(JOIN_SESSION, app.can_start_session());
        menu.set_enabled(UNDO, app.session.is_some() || app.undo_depth() > 0); menu.set_enabled(REDO, app.redo_depth() > 0);
        if redraw {
            let whole = Clip::new(0,0,win.width() as i32,win.height() as i32);
            let r = app.damage.unwrap_or(whole);
            let mut damage = Clip::new(r.x.min(whole.w),r.y.min(whole.h),
                (r.x+r.w).min(whole.w)-r.x.min(whole.w),
                (r.y+r.h).min(whole.h)-r.y.min(whole.h));
            if damage.is_empty() { damage = whole; }
            // ACQUIRE's replay is repair of this back buffer, not new frame
            // damage. Include it when drawing, but commit only this input's delta.
            let acquired = win.acquire(u64::MAX).expect("Gus Kit acquire");
            app.damage = Some(if acquired.replay_full { whole } else {
                acquired.replay.iter().fold(damage, |r,b| union(r,Clip::new(b.x,b.y,b.w,b.h)))
            });
            if let Ok(surface) = win.surface() { render(surface, &mut app, &menu, &mut font, font_size); }
            publish(&mut win, &app, &menu, font_size);
            win.commit(&[libbreenix::window::abi::Rect::new(damage.x,damage.y,damage.w,damage.h)])
                .expect("Gus Kit present");
        }
        if app.quit { break; }
        redraw = false;
        app.damage = None;
        let mut pointer_motion = false;
        let mut reload_font = false;
        let mut resize = None;
        let collaborating=!app.can_start_session();
        let mut dispatch = |delivered: &breengel::Delivered| {
            pointer_motion |= matches!(delivered.event, Event::MouseMove { .. });
            match delivered.event {
                Event::ResizeRequested { width, height } => { resize = Some((width, height)); app.damage = None; redraw = true; return Handled::Redraw; }
                Event::Resized { width, height } => {
                    if initial_resize.take() != Some((width, height)) { app.resize_view(width, height); }
                    app.damage = None; redraw = true; return Handled::Redraw;
                }
                _ => {}
            }
            if matches!(delivered.event, Event::FontChanged) { reload_font = true; app.damage = None; redraw = true; return Handled::Redraw; }
            dispatch_input(&mut app, &mut menu, delivered, &mut redraw)
        };
        let result=if collaborating { win.dispatch(&mut dispatch) } else { win.wait_dispatch(100_000_000,&mut dispatch) };
        let count=result.unwrap_or_else(|e| { if matches!(e, libbreenix::error::Error::Os(libbreenix::errno::Errno::ETIMEDOUT)) { 0 } else { eprintln!("Gus Kit dispatch: {e}"); std::process::exit(1) } });
        if let Some((width, height)) = resize {
            match win.apply_resize(width, height) {
                Ok(()) => app.resize_view(width, height),
                Err(e) => eprintln!("Gus Kit resize refused: {width}x{height}: {e}"),
            }
        }
        if !collaborating {
            if let Some(session) = &app.session {
                let next_title = session_title(&session.core.model.name);
                if next_title != title {
                    libbreenix::window::set_title(win.id(), next_title.as_bytes()).unwrap_or_else(|e| {
                        eprintln!("Gus Kit title: {e}"); std::process::exit(1)
                    });
                    title = next_title;
                }
            }
        }
        if reload_font {
            win.put_display_font(font.take());
            font = win.take_display_font();
            font_size = win.display_size();
        }
        if let Some(session)=&mut app.session {
            let status=session.core.model.status.clone();
            if session.poll(10) { redraw = true; app.damage = None; }
            if session.core.model.status != status { app.message.clear(); }
            app.width=session.core.model.width(); app.height=session.core.model.height();
        }
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
    fn failed_and_ended_joins_allow_another_session_without_losing_the_canvas() {
        for end in [None, Some(libcollab::wire::Message::JoinRefused { reason: b"full".to_vec() }), Some(libcollab::wire::Message::Bye)] {
            let image = Canvas::new(20, 20);
            let mut app = App { damage: None, layout: Layout::new(1), width:20,height:20,clean:image.digest(),history:History::new(image),
                tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
                confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,
                session:Some(collab::session_fixture(collab::Core::join("Peer"))),session_dialog:None };
            app.action(JOIN_SESSION);
            assert!(app.session_dialog.is_none(), "a pending join still owns its session");
            let core = &mut app.session.as_mut().unwrap().core;
            core.model.canvas.pencil(&[(1, 1)], [3, 4, 5]);
            let digest = core.model.canvas.digest();
            match end {
                None => core.closed(1, "send"),
                Some(message) => core.receive(1, &libcollab::wire::encode(&message).unwrap()),
            }
            assert_eq!(core.model.state, "disconnected");
            for action in [HOST_SESSION, JOIN_SESSION] {
                app.action(action);
                assert!(app.session_dialog.is_some(), "ended joins must allow Host and Join");
                assert_eq!(app.canvas().digest(), digest, "keep the drawing available for saving");
                app.session_dialog = None;
            }
        }
    }

    #[test]
    fn session_dialog_scaled_centres_focus_the_drawn_fields() {
        let image = Canvas::new(200, 120);
        let mut app = App { damage: None, layout: Layout::new(2), width:200,height:120,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
        let mut menu = menu();
        app.action(JOIN_SESSION);
        for (r, address) in [(app.layout.name, false), (app.layout.address, true)] {
            let event = Delivered { serial:1, time_ns:1, clicks:1, event:Event::MouseButton {
                button:breengel::buttons::LEFT, pressed:true, x:r.x+r.w/2, y:r.y+r.h/2 } };
            handle(&mut app, &mut menu, &event);
            let dialog = app.session_dialog.as_ref().unwrap();
            assert_eq!(dialog.address.rect, app.layout.logical(app.layout.address));
            assert_eq!(dialog.name.rect, app.layout.logical(app.layout.name));
            assert_eq!(dialog.address.is_focused(), address);
            assert_eq!(dialog.name.is_focused(), !address);
        }
    }

    #[test]
    fn session_dialog_inserts_physical_printable_keys_once() {
        let image = Canvas::new(200, 120);
        let mut app = App { damage: None, layout: Layout::new(1), width:200,height:120,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
        let mut menu = menu();
        app.action(JOIN_SESSION);
        // bwm sends KeyPress, Text and KeyRelease for a physical printable key.
        for (keycode, ascii) in [(2, b'1'), (11, b'0'), (52, b'.'), (11, b'0'),
            (52, b'.'), (3, b'2'), (52, b'.'), (3, b'2'), (15, b'\t'),
            (25, b'P'), (18, b'e'), (18, b'e'), (19, b'r')]
        {
            let modifiers = Modifiers { shift: ascii.is_ascii_uppercase(), ..Modifiers::default() };
            for event in [
                Event::KeyPress { keycode, ascii, text:(ascii as char).to_string(), modifiers, repeat:false },
                Event::Text { text:(ascii as char).to_string() },
                Event::KeyRelease { keycode, modifiers },
            ] {
                handle(&mut app, &mut menu, &Delivered { serial:1,time_ns:1,clicks:0,event });
            }
        }
        let dialog = app.session_dialog.as_mut().unwrap();
        assert_eq!(dialog.address.text(), "10.0.2.2");
        assert_eq!(dialog.name.text(), "Peer");
        assert_eq!(dialog.model.submit(dialog.address.text(), dialog.name.text()),
            session_dialog::Outcome::Start(false, [10, 0, 2, 2], 7890, "Peer".into()));
    }

    #[test]
    fn session_dialog_rejects_bad_address_after_tab_and_enter_text() {
        let image = Canvas::new(200, 120);
        let mut app = App { damage: None, layout: Layout::new(1), width:200,height:120,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
        let mut menu = menu();
        app.action(JOIN_SESSION);
        // App::type_text sends one TEXT source (libuitest app.rs/pointer_live.rs).
        // bwm/input_keys.rs emits Key, Text, then KeyRelease for key(15/28).
        // breengel/event.rs decodes CR's KeyPress ascii as 0, keeping its text.
        let events = [
            Event::Text { text: "bad address".into() },
            Event::KeyPress { keycode:15,ascii:b'\t',text:"\t".into(),modifiers:Modifiers::default(),repeat:false },
            Event::Text { text: "\t".into() },
            Event::KeyRelease { keycode:15,modifiers:Modifiers::default() },
            Event::Text { text: "Peer".into() },
            Event::KeyPress { keycode:28,ascii:0,text:"\r".into(),modifiers:Modifiers::default(),repeat:false },
            Event::Text { text: "\r".into() },
            Event::KeyRelease { keycode:28,modifiers:Modifiers::default() },
        ];
        for (index, event) in events.into_iter().enumerate() {
            handle(&mut app, &mut menu, &Delivered { serial:index as u64 + 1,time_ns:1,clicks:0,event });
        }
        let dialog = app.session_dialog.as_ref().unwrap();
        let nodes = node::session_dialog(false, dialog.address.text(), dialog.name.text(),
            dialog.model.status(), dialog.address.is_focused(), dialog.name.is_focused(),
            [app.layout.session.as_tree_rect(), app.layout.address.as_tree_rect(), app.layout.name.as_tree_rect()]);
        let published = nodes.iter().find(|n| n.key == "session-dialog").unwrap();
        assert_eq!(published.value["status"], "Enter a valid IPv4 address",
            "address={:?}, name={:?}, name_focused={}",
            dialog.address.text(), dialog.name.text(), dialog.name.is_focused());
        assert_eq!(dialog.address.text(), "bad address");
        assert_eq!(dialog.name.text(), "Peer");
        assert!(dialog.name.is_focused());
        assert!(app.session.is_none());
    }

    #[test]
    fn save_as_navigates_and_rejects_unsupported_names() {
        let dir = std::env::temp_dir().join(format!("guskit-save-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let image = Canvas::new(8, 8);
        let mut app = App { damage: None, layout: Layout::new(1), width:8,height:8,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:Some(dir.join("a.png")),message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
        let mut menu = menu();
        let key = |app: &mut App, menu: &mut MenuBar, keycode, ascii, modifiers| {
            let event = Delivered { serial:1,time_ns:1,clicks:0, event:Event::KeyPress {
                ascii, keycode, modifiers, text:String::new(), repeat:false } };
            handle(app, menu, &event);
        };
        let shortcut = Modifiers { ctrl:true, shift:true, ..Modifiers::default() };
        key(&mut app, &mut menu, 31, 0, shortcut);
        assert!(app.picker.is_some(), "Save As opens the picker");
        key(&mut app, &mut menu, 108, 0, Modifiers::default());
        key(&mut app, &mut menu, 28, 0, Modifiers::default());
        assert_eq!(app.directory, dir.join("sub"));
        key(&mut app, &mut menu, 15, 0, Modifiers::default());
        for byte in b"/b.png" { key(&mut app, &mut menu, 0, *byte, Modifiers::default()); }
        key(&mut app, &mut menu, 28, 0, Modifiers::default());
        let path = dir.join("sub/b.png");
        assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(app.file.as_ref(), Some(&path));
        assert_eq!(app.message, "Saved");
        key(&mut app, &mut menu, 31, 0, shortcut);
        key(&mut app, &mut menu, 15, 0, Modifiers::default());
        for byte in b"/bad.txt" { key(&mut app, &mut menu, 0, *byte, Modifiers::default()); }
        key(&mut app, &mut menu, 28, 0, Modifiers::default());
        assert!(!dir.join("sub/bad.txt").exists());
        assert_ne!(app.message, "Saved");
        assert_eq!(app.file.as_ref(), Some(&path));
        key(&mut app, &mut menu, 31, 0, shortcut);
        key(&mut app, &mut menu, 15, 0, Modifiers::default());
        key(&mut app, &mut menu, 102, 0, Modifiers::default());
        key(&mut app, &mut menu, 107, 0, Modifiers { shift:true, ..Modifiers::default() });
        let target = dir.join("text.png");
        let event = Delivered { serial:2, time_ns:2, clicks:0, event:Event::Text { text:target.to_string_lossy().into_owned() } };
        handle(&mut app, &mut menu, &event);
        key(&mut app, &mut menu, 28, 0, Modifiers::default());
        assert!(target.exists(), "text input saves to the submitted path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn passive_events_acknowledge_without_repainting_and_strokes_still_preview() {
        let image = Canvas::new(200, 120);
        let mut app = App { damage: None, layout: Layout::new(1), width:200,height:120,clean:image.digest(),history:History::new(image),
            tool:0,size:0,hue:0,sat:0,val:0,stroke:None,selected_color:None,recent:[[0;3];8],
            confirm:false,quit:false,file:None,message:String::new(),picker:None,directory:PathBuf::from("."),save_as:false,palette:None,canvas_surface:None,session:None,session_dialog:None };
        let mut menu = menu();
        let mut redraw = false;
        let mut event = Delivered { serial:1,time_ns:1,clicks:0,
            event:Event::KeyRelease { keycode:25,modifiers:Modifiers::default() } };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::NoChange);
        event.event = Event::Text { text:"p".into() };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::NoChange);
        event.event = Event::MouseMove { x:20,y:186 };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::NoChange);
        event.event = Event::MouseButton { button:breengel::buttons::LEFT,pressed:true,x:20,y:186 };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::Redraw);
        assert!(redraw && app.damage.is_none(), "press redraws the frame");
        redraw = false;
        event.event = Event::MouseMove { x:40,y:186 };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::Redraw);
        assert_eq!(app.damage, Some(Clip::new(20,186,21,1)), "motion damage is the union of the old and new preview");
        assert_eq!(app.undo_depth(), 0, "preview must not commit the stroke");
        event.event = Event::MouseButton { button:breengel::buttons::LEFT,pressed:false,x:40,y:186 };
        assert_eq!(dispatch_input(&mut app, &mut menu, &event, &mut redraw), Handled::Redraw);
        assert!(redraw && app.damage.is_none(), "release restores full damage for changed status/history");
        assert_eq!(app.undo_depth(), 1);
    }
}
