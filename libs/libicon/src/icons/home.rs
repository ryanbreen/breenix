//! Home icon — animated house with chimney smoke and glowing door.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::sin_approx;
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::particles::{Particle, ParticlePool, Rng};
use crate::physics::Spring;

pub struct HomeIcon {
    base: IconBase,
    /// Overall icon scale driven by spring.
    scale: Spring,
    /// Chimney smoke particles.
    particles: ParticlePool,
    /// RNG for smoke randomness.
    rng: Rng,
    /// Accumulated time for smoke emission throttle (ms).
    smoke_accum: u32,
    /// Is the door currently "open" (warm light visible)?
    door_open: bool,
    /// Progress of the door-open effect 0.0..=1.0.
    door_progress: f32,
}

impl HomeIcon {
    pub fn new() -> Self {
        Self {
            base: IconBase::new(),
            scale: Spring::new(1.0, 300.0, 20.0),
            particles: ParticlePool::new(8),
            rng: Rng::new(0x48_4F4D_45), // "HOME"
            smoke_accum: 0,
            door_open: false,
            door_progress: 0.0,
        }
    }

    fn emit_smoke(&mut self, _cx: f32, _cy: f32, _size: f32) {
        // Spawn just above the chimney top so the wisp is clearly emerging.
        // Chimney center x = 0.175, top y = -0.60 in normalized coords.
        // Start a bit above (-0.64) so the wisp is visible from the start.
        let chimney_x = 0.175;
        let spawn_y = -0.64;

        // Gentle upward drift with slight x wobble.
        // At size=40, vy=-0.15 → ~6 px/s upward. Visible but not a rocket.
        let vx = self.rng.range(-0.03, 0.03);
        let vy = self.rng.range(-0.18, -0.10);
        self.particles.emit(Particle {
            x: chimney_x + self.rng.range(-0.01, 0.01),
            y: spawn_y,
            vx,
            vy,
            life: 1.0,
            max_life: self.rng.range(1.0, 1.8),
            size: self.rng.range(2.0, 3.5),
            color: Color::rgb(180, 180, 195),
            gravity: 0.0,
            friction: 0.4,
        });
    }

}

impl Icon for HomeIcon {
    fn update(&mut self, dt_us: u32, mouse: IconMouse) {
        let state_changed = self.base.update(dt_us, &mouse);
        let dt = dt_us as f32 / 1_000_000.0;

        // On entering Clicked, open the door.
        if state_changed && self.base.state == IconState::Clicked {
            self.door_open = true;
            self.door_progress = 0.0;
        }

        // Advance door animation.
        if self.door_open {
            self.door_progress = (self.door_progress + dt * 3.0).min(1.0);
            // Door closes after Clicked state ends (progress resets when idle).
        }
        if self.base.state == IconState::Idle || self.base.state == IconState::HoverOut {
            self.door_open = false;
            self.door_progress = 0.0;
        }

        // Emit chimney smoke while hovering or after click.
        let emit_smoke = matches!(
            self.base.state,
            IconState::HoverIn | IconState::Hovering | IconState::Clicked
        );
        if emit_smoke {
            self.smoke_accum += dt_us;
            // ~2 wisps per second. Gentle, not a cannon.
            let smoke_interval: u32 = 500_000;
            while self.smoke_accum >= smoke_interval {
                self.smoke_accum -= smoke_interval;
                // Emit relative to icon center (0,0); scaled by size in draw.
                self.emit_smoke(0.0, 0.0, 1.0);
            }
        } else {
            self.smoke_accum = 0;
        }

        // Spring targets.
        let target_scale = match self.base.state {
            IconState::Pressed => 0.9,
            IconState::HoverIn | IconState::Hovering => 1.1,
            IconState::Clicked => 1.15,
            _ => 1.0,
        };
        self.scale.set_target(target_scale);
        self.scale.update(dt);
        self.particles.update(dt);
    }

    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32) {
        let sc = self.scale.value;
        // Idle breathing: gentle ±1% pulse.
        let breathe = 1.0 + sin_approx(self.base.idle_time as f32 / 1_000_000.0 * 1.2) * 0.01;
        let effective_scale = sc * breathe;
        let s = (size as f32 * effective_scale) as i32;
        // Avoid degenerate sizes.
        if s < 4 {
            return;
        }

        // Geometry constants (all relative to scaled size s):
        //   house body: width = 0.8s, height = 0.4s, centred, bottom at cy + 0.3s
        //   roof: triangle from (cx - 0.4s, cy - 0.1s) to apex (cx, cy - 0.5s) to (cx + 0.4s, cy - 0.1s)
        //   door: centred at cx, 0.2s wide, 0.25s tall, bottom at cy + 0.3s
        //   chimney: 0.1s wide, 0.2s tall, right side of roof near peak

        let body_w = s * 8 / 10;
        let body_h = s * 4 / 10;
        let body_x = cx - body_w / 2;
        let body_y = cy - body_h / 2 + s / 10; // centre of body shifted down

        // Roof apex is above body top.
        let apex_y = body_y - s * 4 / 10;
        let roof_left_x = body_x - s / 20;
        let roof_right_x = body_x + body_w + s / 20;

        // Door.
        let door_w = s * 2 / 10;
        let door_h = s * 25 / 100;
        let door_x = cx - door_w / 2;
        let door_y = body_y + body_h - door_h;

        // Chimney (right side, near roof).
        let chimney_w = s / 10;
        let chimney_h = s * 2 / 10;
        let chimney_x = cx + s / 8;
        let chimney_y = apex_y - chimney_h / 2;

        // --- Warm ambient glow behind the house during click ---
        if self.door_open && self.door_progress > 0.1 {
            let glow_t = (self.door_progress - 0.1) / 0.9;
            let glow_w = body_w + (glow_t * s as f32 * 0.3) as i32;
            let glow_h = body_h + (glow_t * s as f32 * 0.2) as i32;
            let glow_x = cx - glow_w / 2;
            let glow_y = body_y + body_h / 2 - glow_h / 2;
            let intensity = (glow_t * 40.0) as u8;
            shapes::fill_rect(fb, glow_x, glow_y, glow_w, glow_h,
                Color::rgb(intensity, intensity / 2, 0));
        }

        // --- Draw house body ---
        shapes::fill_rect(fb, body_x, body_y, body_w, body_h, Color::rgb(120, 160, 200));

        // --- Draw roof (two lines forming an inverted V) ---
        let roof_color = Color::rgb(180, 80, 60);
        shapes::draw_line(fb, roof_left_x, body_y, cx, apex_y, roof_color);
        shapes::draw_line(fb, cx, apex_y, roof_right_x, body_y, roof_color);
        // Give the roof some thickness with a second pass offset by 1px.
        shapes::draw_line(fb, roof_left_x, body_y + 1, cx, apex_y + 1, roof_color);
        shapes::draw_line(fb, cx, apex_y + 1, roof_right_x, body_y + 1, roof_color);

        // --- Draw chimney ---
        shapes::fill_rect(fb, chimney_x, chimney_y, chimney_w, chimney_h, Color::rgb(140, 90, 70));

        // --- Draw door ---
        let door_color = if self.door_open {
            // Warm golden light colour blended with door progress.
            let t = self.door_progress;
            Color::rgb(
                (160.0 + 95.0 * t) as u8,
                (120.0 + 80.0 * t) as u8,
                (60.0 - 40.0 * t.min(1.0)) as u8,
            )
        } else {
            Color::rgb(160, 120, 60)
        };
        shapes::fill_rect(fb, door_x, door_y, door_w, door_h, door_color);

        // Warm glow around door when open — double outline for soft glow effect.
        if self.door_open && self.door_progress > 0.3 {
            let t = (self.door_progress - 0.3) / 0.7;
            let a1 = (t * 80.0) as u8;
            let a2 = (t * 40.0) as u8;
            shapes::draw_rect(fb, door_x - 1, door_y - 1, door_w + 2, door_h + 2,
                Color::rgb(a1, a1 / 2, 0));
            shapes::draw_rect(fb, door_x - 2, door_y - 2, door_w + 4, door_h + 4,
                Color::rgb(a2, a2 / 2, 0));
        }

        // --- Draw window (left of door) ---
        let win_size = s * 12 / 100;
        let win_x = cx - body_w / 4 - win_size / 2;
        let win_y = body_y + body_h / 4;
        let win_color = if self.door_open {
            let t = self.door_progress;
            Color::rgb(
                (80.0 + 175.0 * t) as u8,
                (90.0 + 140.0 * t) as u8,
                (110.0 - 60.0 * t.min(1.0)) as u8,
            )
        } else {
            Color::rgb(80, 90, 110)
        };
        shapes::fill_rect(fb, win_x, win_y, win_size, win_size, win_color);
        // Window cross dividers.
        let cross_color = Color::rgb(100, 130, 160);
        shapes::fill_rect(fb, win_x + win_size / 2, win_y, 1, win_size, cross_color);
        shapes::fill_rect(fb, win_x, win_y + win_size / 2, win_size, 1, cross_color);

        // --- Draw smoke particles (offset by icon center) ---
        // Particles were emitted with unit-normalised coords; scale by actual size.
        // Because emit_smoke used (0,0) and size=1.0, particle x/y ARE already
        // in screen coords relative to (0,0) when size=1.0.  We need to rescale.
        // Actually we passed (0,0) as cx/cy and 1.0 as size, so particle positions
        // encode the normalised offset.  Multiply by actual size and add (cx, cy).
        // Chimney normalised offset: (0.15, -0.52).  With scale applied:
        // px_screen = particle.x * size + cx  (but particles encode screen coords from emit).
        // Re-examine: emit_smoke(0.0, 0.0, 1.0) → chimney_x = 0 + 1*0.15 = 0.15.
        // So particle.x starts near 0.15 (plus random drift).
        // We want: draw offset = (cx + particle.x * size, cy + particle.y * size)
        // The ParticlePool::draw adds (offset_x, offset_y) to particle.(x,y) as i32.
        // So pass offset = (cx, cy) but scale particle coords by size first.
        // Since we can't rescale in-place (pool is immutable in draw), instead we
        // draw manually with the scale factor.
        let smoke_scale = size as f32;
        for p in &self.particles.particles {
            if !p.alive() {
                continue;
            }
            let alpha = p.life;
            let r = (p.color.r as f32 * alpha * 0.8) as u8;
            let g = (p.color.g as f32 * alpha * 0.8) as u8;
            let b = (p.color.b as f32 * alpha * 0.8) as u8;
            let pc = Color::rgb(r, g, b);
            let px = (p.x * smoke_scale) as i32 + cx;
            let py = (p.y * smoke_scale) as i32 + cy;
            let sz = ((p.size * (0.5 + 0.5 * p.life)) as i32).max(1);
            shapes::fill_rect(fb, px - sz / 2, py - sz / 2, sz, sz, pc);
        }
    }

    fn bounds_overflow(&self) -> i32 {
        24
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.particles.clear();
        self.smoke_accum = 0;
        self.door_open = false;
        self.door_progress = 0.0;
    }

    fn name(&self) -> &'static str {
        "Home"
    }
}
