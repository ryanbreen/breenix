//! Animated graphics demo.
use breengel::{Event, Handled, Window};
use libgfx::{color::Color, surface::Surface, shapes};
#[path = "demos/status.rs"] mod status;
/// Pre-computed sine table (0-359 degrees, scaled by 1000)
const SIN_TABLE: [i32; 360] = [
    0, 17, 35, 52, 70, 87, 105, 122, 139, 156, 174, 191, 208, 225, 242, 259, 276, 292, 309, 326,
    342, 358, 375, 391, 407, 423, 438, 454, 469, 485, 500, 515, 530, 545, 559, 574, 588, 602, 616,
    629, 643, 656, 669, 682, 695, 707, 719, 731, 743, 755, 766, 777, 788, 799, 809, 819, 829, 839,
    848, 857, 866, 875, 883, 891, 899, 906, 914, 921, 927, 934, 940, 946, 951, 956, 961, 966, 970,
    974, 978, 982, 985, 988, 990, 993, 995, 996, 998, 999, 999, 1000, 1000, 1000, 999, 999, 998,
    996, 995, 993, 990, 988, 985, 982, 978, 974, 970, 966, 961, 956, 951, 946, 940, 934, 927, 921,
    914, 906, 899, 891, 883, 875, 866, 857, 848, 839, 829, 819, 809, 799, 788, 777, 766, 755, 743,
    731, 719, 707, 695, 682, 669, 656, 643, 629, 616, 602, 588, 574, 559, 545, 530, 515, 500, 485,
    469, 454, 438, 423, 407, 391, 375, 358, 342, 326, 309, 292, 276, 259, 242, 225, 208, 191, 174,
    156, 139, 122, 105, 87, 70, 52, 35, 17, 0, -17, -35, -52, -70, -87, -105, -122, -139, -156,
    -174, -191, -208, -225, -242, -259, -276, -292, -309, -326, -342, -358, -375, -391, -407, -423,
    -438, -454, -469, -485, -500, -515, -530, -545, -559, -574, -588, -602, -616, -629, -643, -656,
    -669, -682, -695, -707, -719, -731, -743, -755, -766, -777, -788, -799, -809, -819, -829, -839,
    -848, -857, -866, -875, -883, -891, -899, -906, -914, -921, -927, -934, -940, -946, -951, -956,
    -961, -966, -970, -974, -978, -982, -985, -988, -990, -993, -995, -996, -998, -999, -999, -1000,
    -1000, -1000, -999, -999, -998, -996, -995, -993, -990, -988, -985, -982, -978, -974, -970,
    -966, -961, -956, -951, -946, -940, -934, -927, -921, -914, -906, -899, -891, -883, -875, -866,
    -857, -848, -839, -829, -819, -809, -799, -788, -777, -766, -755, -743, -731, -719, -707, -695,
    -682, -669, -656, -643, -629, -616, -602, -588, -574, -559, -545, -530, -515, -500, -485, -469,
    -454, -438, -423, -407, -391, -375, -358, -342, -326, -309, -292, -276, -259, -242, -225, -208,
    -191, -174, -156, -139, -122, -105, -87, -70, -52, -35, -17,
];

/// Get sine value (scaled by 1000) for angle in degrees
fn sin(angle: i32) -> i32 {
    let a = ((angle % 360) + 360) % 360;
    SIN_TABLE[a as usize]
}

/// Get cosine value (scaled by 1000) for angle in degrees
fn cos(angle: i32) -> i32 {
    sin(angle + 90)
}

/// Bouncing ball state
struct Ball {
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    radius: i32,
    color: Color,
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: Color) -> Self {
        Self { x, y, vx, vy, radius, color }
    }

    fn update(&mut self, width: i32, height: i32) {
        self.x += self.vx;
        self.y += self.vy;

        if self.x - self.radius < 0 {
            self.x = self.radius;
            self.vx = -self.vx;
        }
        if self.x + self.radius >= width {
            self.x = width - self.radius - 1;
            self.vx = -self.vx;
        }
        if self.y - self.radius < 0 {
            self.y = self.radius;
            self.vy = -self.vy;
        }
        if self.y + self.radius >= height {
            self.y = height - self.radius - 1;
            self.vy = -self.vy;
        }
    }

    fn draw(&self, fb: &mut Surface) {
        shapes::fill_circle(fb, self.x, self.y, self.radius, self.color);
    }
}

/// Convert hue (0-359) to RGB color
fn hue_to_rgb(hue: u32) -> Color {
    let h = hue % 360;
    let x = (255 * (60 - (h % 60).min(60 - (h % 60)))) / 60;

    match h / 60 {
        0 => Color::rgb(255, x as u8, 0),
        1 => Color::rgb(x as u8, 255, 0),
        2 => Color::rgb(0, 255, x as u8),
        3 => Color::rgb(0, x as u8, 255),
        4 => Color::rgb(x as u8, 0, 255),
        _ => Color::rgb(255, 0, x as u8),
    }
}

/// Draw rotating lines from center
fn draw_rotating_lines(fb: &mut Surface, cx: i32, cy: i32, radius: i32, angle: i32, num_lines: i32) {
    for i in 0..num_lines {
        let a = angle + (i * 360 / num_lines);
        let x2 = cx + (radius * cos(a)) / 1000;
        let y2 = cy + (radius * sin(a)) / 1000;

        let hue = ((a % 360) + 360) % 360;
        let color = hue_to_rgb(hue as u32);
        shapes::draw_line(fb, cx, cy, x2, y2, color);
    }
}

/// Draw pulsing rectangles
fn draw_pulsing_rects(fb: &mut Surface, cx: i32, cy: i32, frame: i32) {
    let pulse = (sin(frame * 3) + 1000) / 20;

    for i in 0..5 {
        let size = (20 + i * 15 + pulse / 5).min(cx).min(cy).max(0);
        let alpha = 255 - i * 40;
        let color = Color::rgb(alpha as u8, (100 + i * 30) as u8, (200 - i * 20) as u8);
        shapes::draw_rect(fb, cx - size, cy - size, size * 2, size * 2, color);
    }
}

/// Draw wave pattern
fn draw_wave(fb: &mut Surface, y_base: i32, width: i32, frame: i32, color: Color) {
    let mut prev_y = y_base + (sin(frame) * 30) / 1000;

    let mut x = 0;
    while x < width {
        let phase = frame + x * 2;
        let y = y_base + (sin(phase) * 30) / 1000;
        shapes::draw_line(fb, x - 4, prev_y, x, y, color);
        prev_y = y;
        x += 4;
    }
}


fn main() {
    let mut win = Window::new_without_fonts(b"Demo", 400, 300).expect("window");
    let mut balls = [Ball::new(100,100,3,2,20,Color::rgb(255,100,100)), Ball::new(200,150,-2,3,15,Color::rgb(100,255,100)), Ball::new(150,200,2,-2,25,Color::rgb(100,100,255)), Ball::new(300,100,-3,-2,18,Color::rgb(255,255,100))];
    let mut frame = 0i32; let mut epochs = 0u64; let mut last = None; let mut remainder = 0u64; let mut redraw = true;
    win.set_animating(true);
    loop {
        if !redraw {
            let mut close = false; let mut resize = None; let mut input = false;
            let win_width = win.width() as i32; let win_height = win.height() as i32;
            let count = win.wait_dispatch(100_000_000, |d| {
                input |= d.serial != 0;
                match d.event {
                    Event::AnimationTick { time_ms } => {
                        let elapsed = remainder + last.map_or(0, |t: u64| time_ms.saturating_sub(t));
                        last = Some(time_ms); remainder = elapsed % 16;
                        for _ in 0..(elapsed / 16).min(1000) { frame = (frame + 1) % 360; epochs = epochs.saturating_add(1); for ball in &mut balls { ball.update(win_width, win_height); } }
                        redraw = true;
                    }
                    Event::CloseRequested => close = true,
                    Event::ResizeRequested { width, height } => { resize = Some((width,height)); redraw = true; }
                    Event::Resized { .. } => redraw = true,
                    _ => {}
                }
                if redraw { Handled::Redraw } else { Handled::NoChange }
            }).expect("dispatch");
            if close { return; }
            if let Some((w,h)) = resize { let _ = win.apply_resize(w,h); }
            if !redraw { if count != 0 && input { let _ = win.commit_no_change(); } continue; }
        }
        redraw = false;
        let width = win.width() as i32; let height = win.height() as i32;
        let cx = width/2; let cy = height/2;
        let radius = 80.min(cx).min(cy).max(0);
        let mut objects = Vec::new();
        for i in 0..12 { let a = frame*2 + i*30; let x = cx + radius*cos(a)/1000; let y = cy + radius*sin(a)/1000; objects.push(status::Object { bounds: [cx.min(x) as i64, cy.min(y) as i64, (cx-x).abs() as i64, (cy-y).abs() as i64], value: serde_json::json!({}) }); }
        for i in 0..5 { let size = 20 + i*15 + (sin(frame*3)+1000)/100; let size = size.min(cx).min(cy).max(0); objects.push(status::Object { bounds: [(cx-size) as i64,(cy-size) as i64,(size*2) as i64,(size*2) as i64], value: serde_json::json!({}) }); }
        win.set_tree(status::build("demo",100,epochs,true,win.width(),win.height(),"object",&objects));
        let fb = win.surface().expect("surface"); fb.clear(Color::rgb(10,20,40));
        draw_rotating_lines(fb,cx,cy,radius,frame*2,12);
        draw_pulsing_rects(fb,cx,cy,frame);
        draw_wave(fb,height-30,width,frame*3,Color::rgb(0,150,255));
        for ball in &balls { ball.draw(fb); }
        win.present().expect("present");
    }
}
