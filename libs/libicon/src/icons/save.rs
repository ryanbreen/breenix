//! Save icon — floppy disk with tilt-to-cursor hover, stamp pulse on click.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::sin_approx;
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::particles::{Particle, ParticlePool, Rng};
use crate::physics::Spring;

pub struct SaveIcon {
    base: IconBase,
    /// Overall scale.
    scale: Spring,
    /// Tilt offset: positive = tilted right (shear on x-axis, pixels).
    tilt: Spring,
    /// Float bob offset (pixels, vertical).
    float_y: Spring,
    /// Stamp pulse rings: (birth_time_ms, alive).
    stamp_rings: [(u32, bool); 2],
    /// Time elapsed since click started (ms).
    click_time: u32,
    /// Sparkle particles at cardinal points.
    particles: ParticlePool,
    rng: Rng,
}

impl SaveIcon {
    pub fn new() -> Self {
        Self {
            base: IconBase::new(),
            scale: Spring::new(1.0, 320.0, 22.0),
            tilt: Spring::new(0.0, 250.0, 15.0),
            float_y: Spring::new(0.0, 180.0, 12.0),
            stamp_rings: [(0, false); 2],
            click_time: 0,
            particles: ParticlePool::new(24),
            rng: Rng::new(0x5AFE_D15C),
        }
    }

    fn emit_stamp_sparkles(&mut self, size: f32) {
        // 4 cardinal sparkles burst outward.
        let dirs: [(f32, f32); 4] = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
        for (dx, dy) in dirs {
            let speed = self.rng.range(30.0, 55.0) * (size / 32.0);
            self.particles.emit(Particle {
                x: dx * 0.1,
                y: dy * 0.1,
                vx: dx * speed,
                vy: dy * speed,
                life: 1.0,
                max_life: self.rng.range(0.35, 0.6),
                size: self.rng.range(2.0, 3.5),
                color: Color::rgb(255, 220, 100),
                gravity: 0.0,
                friction: 2.5,
            });
        }
    }
}

impl Icon for SaveIcon {
    fn update(&mut self, dt_ms: u32, mouse: IconMouse) {
        let state_changed = self.base.update(dt_ms, &mouse);
        let dt = dt_ms as f32 / 1000.0;

        // Stamp effect on click entry.
        if state_changed && self.base.state == IconState::Clicked {
            self.stamp_rings[0] = (0, true);
            self.stamp_rings[1] = (100, true);
            self.click_time = 0;
            self.emit_stamp_sparkles(32.0); // nominal size for speed
        }

        if self.base.state == IconState::Clicked {
            self.click_time += dt_ms;
        }

        // Tilt toward cursor when hovering.
        let target_tilt = match self.base.state {
            IconState::HoverIn | IconState::Hovering => mouse.rel_x * 4.0,
            IconState::Pressed => 0.0,
            _ => 0.0,
        };

        // Float bob when hovering.
        let target_float = match self.base.state {
            IconState::HoverIn | IconState::Hovering => {
                sin_approx(self.base.state_time as f32 / 1000.0 * 4.0) * 2.0
            }
            _ => 0.0,
        };

        // Scale targets.
        let target_scale = match self.base.state {
            IconState::Pressed => 0.88,
            IconState::HoverIn | IconState::Hovering => 1.08,
            IconState::Clicked => 1.15,
            _ => 1.0,
        };

        self.scale.set_target(target_scale);
        self.tilt.set_target(target_tilt);
        self.float_y.set_target(target_float);
        self.scale.update(dt);
        self.tilt.update(dt);
        self.float_y.update(dt);
        self.particles.update(dt);
    }

    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32) {
        let sc = self.scale.value;
        // Idle tilt oscillation.
        let idle_tilt_px =
            sin_approx(self.base.idle_time as f32 / 1000.0 * 0.9) * 1.5;
        let tilt_px = self.tilt.value + idle_tilt_px;
        let float_off = self.float_y.value as i32;

        let s = (size as f32 * sc) as i32;
        if s < 4 {
            return;
        }

        let draw_cy = cy + float_off;
        let half = s / 2;

        // Tilt shear: shift top edge right/left by tilt_px, bottom unchanged.
        // Approximate by drawing a parallelogram: separate top-half and bottom-half.
        let top_x = cx + tilt_px as i32;
        let bot_x = cx;

        // --- Floppy body ---
        // Draw as two rects (top half with offset, bottom half at centre).
        let body_color = Color::rgb(80, 100, 140);
        let body_top_y = draw_cy - half;
        let body_bot_y = draw_cy;
        shapes::fill_rect(fb, top_x - half, body_top_y, s, half, body_color);
        shapes::fill_rect(fb, bot_x - half, body_bot_y, s, half, body_color);

        // --- Notch in top-right corner (background-coloured cutout) ---
        // Approximate floppy notch: small darker triangle in top-right.
        let notch_size = s / 6;
        let notch_x = top_x + half - notch_size;
        let notch_y = body_top_y;
        shapes::fill_rect(fb, notch_x, notch_y, notch_size, notch_size, Color::rgb(30, 35, 50));

        // --- Label area (lighter rectangle, upper-middle of body) ---
        let label_w = s * 6 / 10;
        let label_h = s * 3 / 10;
        let label_x = top_x - label_w / 2;
        let label_y = body_top_y + s / 10;
        shapes::fill_rect(fb, label_x, label_y, label_w, label_h, Color::rgb(220, 220, 220));

        // Small write-protect notch on label (dark rectangle).
        let wp_w = label_w / 5;
        let wp_h = label_h / 3;
        shapes::fill_rect(
            fb,
            label_x + label_w - wp_w - 2,
            label_y + label_h - wp_h - 2,
            wp_w,
            wp_h,
            Color::rgb(100, 100, 110),
        );

        // --- Stamp pulse rings after click ---
        if self.base.state == IconState::Clicked {
            let elapsed = self.click_time;
            for &(birth_ms, alive) in &self.stamp_rings {
                if !alive || elapsed < birth_ms {
                    continue;
                }
                let ring_age = elapsed - birth_ms;
                if ring_age > 400 {
                    continue;
                }
                let t = ring_age as f32 / 400.0;
                let ring_r = (t * 48.0) as i32 + half / 2;
                let intensity = ((1.0 - t) * 220.0) as u8;
                let ring_color = Color::rgb(intensity, (intensity as u32 * 220 / 255) as u8, 0);
                shapes::draw_circle(fb, bot_x, draw_cy, ring_r, ring_color);
            }
        }

        // --- Sparkle particles ---
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
            let py = (p.y * scale) as i32 + draw_cy;
            let sz = ((p.size * (0.5 + 0.5 * p.life)) as i32).max(1);
            shapes::fill_rect(fb, px - sz / 2, py - sz / 2, sz, sz, pc);
        }
    }

    fn bounds_overflow(&self) -> i32 {
        // Stamp rings expand ~48px + sparkles.
        52
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.tilt.impulse(0.0, 0.0);
        self.tilt.set_target(0.0);
        self.float_y.impulse(0.0, 0.0);
        self.float_y.set_target(0.0);
        self.stamp_rings = [(0, false); 2];
        self.click_time = 0;
        self.particles.clear();
    }

    fn name(&self) -> &'static str {
        "Save"
    }
}
