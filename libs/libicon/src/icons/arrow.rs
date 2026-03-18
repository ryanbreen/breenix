//! Arrow icon — animated forward/back arrow with motion-blur trail on click.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::sin_approx;
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::particles::{Particle, ParticlePool, Rng};
use crate::physics::Spring;

pub struct ArrowIcon {
    base: IconBase,
    /// If true the arrow points right (forward); if false, left (back).
    pub forward: bool,
    /// X stretch spring — positive = lean forward, negative = pull back.
    stretch_x: Spring,
    /// Overall scale spring.
    scale: Spring,
    /// Speed-line particles along the travel direction.
    particles: ParticlePool,
    /// RNG for particle variety.
    rng: Rng,
}

impl ArrowIcon {
    pub fn new(forward: bool) -> Self {
        Self {
            base: IconBase::new(),
            forward,
            stretch_x: Spring::new(0.0, 350.0, 18.0),
            scale: Spring::new(1.0, 300.0, 20.0),
            particles: ParticlePool::new(24),
            rng: Rng::new(if forward { 0xF0_1234 } else { 0xBA_CEDC }),
        }
    }

    fn dir(&self) -> f32 {
        if self.forward { 1.0 } else { -1.0 }
    }

    fn emit_speed_lines(&mut self, count: usize) {
        let dir = self.dir();
        for _ in 0..count {
            let vy = self.rng.range(-6.0, 6.0);
            let speed = self.rng.range(50.0, 120.0);
            // Start slightly behind the arrow tip (unit-normalised coords).
            let start_x = -dir * self.rng.range(0.1, 0.4);
            let start_y = self.rng.range(-0.15, 0.15);
            self.particles.emit(Particle {
                x: start_x,
                y: start_y,
                vx: dir * speed,
                vy,
                life: 1.0,
                max_life: self.rng.range(0.2, 0.45),
                size: self.rng.range(1.5, 3.0),
                color: Color::rgb(100, 180, 255),
                gravity: 0.0,
                friction: 2.0,
            });
        }
    }
}

impl Icon for ArrowIcon {
    fn update(&mut self, dt_ms: u32, mouse: IconMouse) {
        let state_changed = self.base.update(dt_ms, &mouse);
        let dt = dt_ms as f32 / 1000.0;
        let dir = self.dir();

        // On click: snap the stretch spring to create a "launch" feel.
        if state_changed && self.base.state == IconState::Clicked {
            // Impulse in travel direction — overshoots via elastic settle.
            self.stretch_x.impulse(self.stretch_x.value, dir * 0.4);
            self.emit_speed_lines(8);
        }

        // Spring targets.
        let (target_scale, target_stretch) = match self.base.state {
            IconState::Pressed => {
                // Compress opposite to travel (slingshot pull-back).
                (0.88, -dir * 0.12)
            }
            IconState::HoverIn => (1.05, dir * 0.08),
            IconState::Hovering => {
                // Oscillate slightly in idle hover.
                let osc = sin_approx(self.base.state_time as f32 / 500.0 * 3.14159265) * 0.03;
                (1.05, dir * (0.08 + osc))
            }
            IconState::Clicked => (1.12, dir * 0.18),
            _ => (1.0, 0.0),
        };

        self.scale.set_target(target_scale);
        self.stretch_x.set_target(target_stretch);
        self.scale.update(dt);
        self.stretch_x.update(dt);
        self.particles.update(dt);
    }

    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32) {
        let sc = self.scale.value;
        let s = (size as f32 * sc) as i32;
        if s < 4 {
            return;
        }

        // Stretch offset in pixels.
        let ox = (self.stretch_x.value * size as f32) as i32;
        let dir = if self.forward { 1 } else { -1 };

        // Arrow shaft: horizontal line from left edge to right edge, centred vertically.
        let shaft_y = cy;
        let left_x = cx - s * 45 / 100 + ox;
        let right_x = cx + s * 45 / 100 + ox;

        // For motion blur on click: draw 3 fading ghost copies behind the arrow.
        if self.base.state == IconState::Clicked || self.base.state == IconState::HoverOut {
            let progress = self.base.state_time as f32 / 500.0;
            if progress < 0.6 {
                for i in 1..=3_i32 {
                    let trail_alpha = (1.0 - progress) * (1.0 - i as f32 / 4.0);
                    let ta = (trail_alpha * 120.0) as u8;
                    let trail_c = Color::rgb(ta / 2, ta, ta);
                    let trail_ox = -dir * i * s / 8;
                    shapes::draw_line(
                        fb,
                        left_x + trail_ox,
                        shaft_y,
                        right_x + trail_ox,
                        shaft_y,
                        trail_c,
                    );
                }
            }
        }

        let arrow_color = Color::rgb(220, 220, 230);

        // Draw shaft (2px thick via two lines).
        shapes::draw_line(fb, left_x, shaft_y, right_x, shaft_y, arrow_color);
        shapes::draw_line(fb, left_x, shaft_y + 1, right_x, shaft_y + 1, arrow_color);

        // Arrowhead: two angled lines forming a chevron at the tip.
        let tip_x = if self.forward { right_x } else { left_x };
        let head_len = s / 5;
        // Upper arm of chevron.
        shapes::draw_line(
            fb,
            tip_x,
            shaft_y,
            tip_x - dir * head_len,
            shaft_y - head_len,
            arrow_color,
        );
        // Lower arm of chevron.
        shapes::draw_line(
            fb,
            tip_x,
            shaft_y + 1,
            tip_x - dir * head_len,
            shaft_y + 1 + head_len,
            arrow_color,
        );

        // Draw speed-line particles (unit-normalised coords → scale by size).
        let scale = size as f32;
        for p in &self.particles.particles {
            if !p.alive() {
                continue;
            }
            let alpha = p.life;
            let r = (p.color.r as f32 * alpha) as u8;
            let g = (p.color.g as f32 * alpha) as u8;
            let b = (p.color.b as f32 * alpha) as u8;
            let pc = Color::rgb(r, g, b);
            let px = (p.x * scale) as i32 + cx;
            let py = (p.y * scale) as i32 + cy;
            shapes::fill_rect(fb, px, py, 1, 1, pc);
        }
    }

    fn bounds_overflow(&self) -> i32 {
        18
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.stretch_x.impulse(0.0, 0.0);
        self.stretch_x.set_target(0.0);
        self.particles.clear();
    }

    fn name(&self) -> &'static str {
        if self.forward {
            "Arrow Forward"
        } else {
            "Arrow Back"
        }
    }
}
