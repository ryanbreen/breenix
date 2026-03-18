//! Save icon — floppy disk insertion animation.
//!
//! A floppy disk sits above a thin drive slot. On hover the disk lifts slightly.
//! On click the disk springs down into the slot (with clipping), the slot glows,
//! sparkles fire from the slot edges, then the spring ejects the disk back out.
//! The disk is drawn as a single solid shape — no tilt, no shear, no splitting.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::sin_approx;
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::particles::{Particle, ParticlePool, Rng};
use crate::physics::Spring;

pub struct SaveIcon {
    base: IconBase,
    /// Overall scale spring. stiffness=320, damping=22.
    scale: Spring,
    /// Vertical lift for hover (negative = up). stiffness=200, damping=14.
    lift_y: Spring,
    /// Insertion depth as a fraction of disk height (0=resting, 0.56=inserted).
    /// stiffness=350, damping=18.
    insert_y: Spring,
    /// Slot glow intensity 0..1, fades after click.
    slot_glow: f32,
    /// Accumulated time in the Clicked state, ms.
    click_time: u32,
    /// True once the eject target (0.0) has been set this cycle.
    ejecting: bool,
    /// True once sparkles have been emitted this click.
    sparkles_emitted: bool,
    particles: ParticlePool,
    rng: Rng,
}

impl SaveIcon {
    pub fn new() -> Self {
        Self {
            base: IconBase::new(),
            scale: Spring::new(1.0, 320.0, 22.0),
            lift_y: Spring::new(0.0, 200.0, 14.0),
            insert_y: Spring::new(0.0, 350.0, 18.0),
            slot_glow: 0.0,
            click_time: 0,
            ejecting: false,
            sparkles_emitted: false,
            particles: ParticlePool::new(16),
            rng: Rng::new(0xD15C_FABB),
        }
    }

    /// Emit sparkle particles at the left and right edges of the drive slot.
    ///
    /// Particle x/y are stored as fractions of `size` centered on the slot
    /// horizontal center; the draw function will anchor them at (cx, slot_y).
    fn emit_slot_sparkles(&mut self) {
        // 3 particles per side shooting outward and slightly upward.
        for &side in &[-1.0_f32, 1.0_f32] {
            for _ in 0..3 {
                // x offset: ±0.4 units (40% of icon size from center) = slot half-width
                let x0 = side * 0.40;
                let speed = self.rng.range(0.04, 0.09); // units/s (will be multiplied by size in draw)
                let angle_jitter = self.rng.range(-0.3, 0.3);
                let vx = side * speed + angle_jitter * speed * 0.4;
                let vy = self.rng.range(-speed * 0.9, -speed * 0.3);
                self.particles.emit(Particle {
                    x: x0,
                    y: 0.0,
                    vx,
                    vy,
                    life: 1.0,
                    max_life: self.rng.range(0.30, 0.55),
                    size: self.rng.range(1.5, 3.0),
                    color: Color::rgb(255, 230, 120),
                    gravity: 0.06, // units/s² downward
                    friction: 2.0,
                });
            }
        }
    }
}

impl Icon for SaveIcon {
    fn update(&mut self, dt_ms: u32, mouse: IconMouse) {
        let state_changed = self.base.update(dt_ms, &mouse);
        let dt = dt_ms as f32 / 1000.0;

        // On entering Clicked: start insertion, light up the slot.
        if state_changed && self.base.state == IconState::Clicked {
            self.click_time = 0;
            self.ejecting = false;
            self.sparkles_emitted = false;
            self.insert_y.set_target(0.56);
            self.slot_glow = 1.0;
        }

        // Manage the click lifecycle: emit sparkles once disk is ~inserted,
        // then eject after 300 ms total.
        if self.base.state == IconState::Clicked {
            self.click_time += dt_ms;

            // Emit sparkles once the disk is clearly into the slot (~100 ms in).
            if self.click_time >= 100 && !self.sparkles_emitted {
                self.sparkles_emitted = true;
                self.emit_slot_sparkles();
            }

            // Start ejecting after 300 ms.
            if self.click_time >= 300 && !self.ejecting {
                self.ejecting = true;
                self.insert_y.set_target(0.0);
            }
        }

        // Fade slot glow (~0.4s decay).
        if self.slot_glow > 0.0 {
            self.slot_glow = (self.slot_glow - dt * 2.5).max(0.0);
        }

        // Scale target.
        let target_scale = match self.base.state {
            IconState::Pressed => 0.92,
            IconState::HoverIn | IconState::Hovering => 1.05,
            _ => 1.0,
        };

        // lift_y target: hover lifts the disk -3 px with a gentle ±2 px bob.
        let target_lift = match self.base.state {
            IconState::HoverIn | IconState::Hovering => {
                let bob = sin_approx(self.base.state_time as f32 / 1000.0 * 3.5) * 2.0;
                -3.0 + bob
            }
            // Pressed: nudge toward slot.
            IconState::Pressed => 2.0,
            // Idle: subtle breathing ~1 px.
            IconState::Idle => {
                sin_approx(self.base.idle_time as f32 / 1000.0 * 1.8) * 1.0
            }
            _ => 0.0,
        };

        // When leaving Clicked, snap insert_y back to 0.
        if self.base.state != IconState::Clicked {
            if state_changed {
                // Clear click bookkeeping on state exit.
                self.ejecting = false;
                self.sparkles_emitted = false;
                self.click_time = 0;
            }
            self.insert_y.set_target(0.0);
        }

        self.scale.set_target(target_scale);
        self.lift_y.set_target(target_lift);
        self.scale.update(dt);
        self.lift_y.update(dt);
        self.insert_y.update(dt);
        self.particles.update(dt);
    }

    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32) {
        let sc = self.scale.value;
        let s = ((size as f32 * sc) as i32).max(4);

        // Disk dimensions.
        let disk_w = s * 7 / 8;
        let disk_h = s * 7 / 8;

        // lift_y offsets the entire disk vertically (negative = up).
        let lift_px = self.lift_y.value as i32;

        // The disk rests centered slightly above cy so the slot fits below it.
        // disk top-y at rest (no insertion).
        let disk_rest_top_y = cy - disk_h / 2 - s / 10 + lift_px;

        // Insertion pushes the disk DOWN.
        let insert_px = (self.insert_y.value * disk_h as f32) as i32;
        let disk_top_y = disk_rest_top_y + insert_px;

        // Visible height: clip bottom of disk as it enters the slot.
        // The "clip plane" is at disk_rest_top_y + disk_h (the slot top).
        // As insert_px grows, visible_h shrinks from disk_h toward 0.
        let visible_h = (disk_h - insert_px).max(0);

        let disk_x = cx - disk_w / 2;
        let body_color = Color::rgb(80, 100, 140);

        // ---------------------------------------------------------------
        // Floppy disk body (clipped)
        // ---------------------------------------------------------------
        if visible_h > 0 {
            shapes::fill_rect(fb, disk_x, disk_top_y, disk_w, visible_h, body_color);

            // --- Label: upper ~30% of disk, rgb(220,220,220) ---
            let label_w = disk_w * 6 / 10;
            let label_h = disk_h * 3 / 10;
            let label_x = cx - label_w / 2;
            let label_y_in_disk = disk_h / 10; // offset from disk top
            let label_abs_y = disk_top_y + label_y_in_disk;
            // Only draw if label starts within the visible region.
            if label_y_in_disk < visible_h {
                let lh = (label_h).min(visible_h - label_y_in_disk).max(0);
                if lh > 0 {
                    shapes::fill_rect(
                        fb,
                        label_x,
                        label_abs_y,
                        label_w,
                        lh,
                        Color::rgb(220, 220, 220),
                    );
                }
            }

            // --- Metal slider: ~bottom 20% of disk, rgb(160,165,175) ---
            let slider_w = disk_w * 4 / 10;
            let slider_h = (disk_h / 8).max(2);
            let slider_x = cx - slider_w / 2;
            let slider_y_in_disk = disk_h - disk_h / 5;
            let slider_abs_y = disk_top_y + slider_y_in_disk;
            if slider_y_in_disk < visible_h {
                let sh = slider_h.min(visible_h - slider_y_in_disk).max(0);
                if sh > 0 {
                    shapes::fill_rect(
                        fb,
                        slider_x,
                        slider_abs_y,
                        slider_w,
                        sh,
                        Color::rgb(160, 165, 175),
                    );
                }
            }

            // --- Notch: top-right corner, rgb(30,35,50) ---
            let notch = (disk_h / 7).max(2);
            let notch_x = disk_x + disk_w - notch;
            if notch < visible_h {
                shapes::fill_rect(
                    fb,
                    notch_x,
                    disk_top_y,
                    notch,
                    notch.min(visible_h),
                    Color::rgb(30, 35, 50),
                );
            }
        }

        // ---------------------------------------------------------------
        // Drive slot — drawn AFTER disk so its face occludes the disk edge
        // ---------------------------------------------------------------
        // Slot sits exactly at the bottom of where the disk rests (ignoring lift).
        let slot_h = ((s / 14).max(2)).min(4);
        let slot_w = disk_w + s / 6; // slightly wider than the disk
        let slot_x = cx - slot_w / 2;
        // Anchor at the rest position of the disk bottom (independent of lift/insert).
        let slot_y = cy - disk_h / 2 - s / 10 + disk_h;

        // Blend slot color toward glow color.
        let glow = self.slot_glow;
        let slot_color = Color::rgb(
            lerp_u8(50, 100, glow),
            lerp_u8(55, 130, glow),
            lerp_u8(65, 180, glow),
        );
        shapes::fill_rect(fb, slot_x, slot_y, slot_w, slot_h, slot_color);

        // Bright highlight lines above/below slot when glow > 0.7.
        if glow > 0.7 {
            let t = (glow - 0.7) / 0.3;
            let hi_color = Color::rgb(
                lerp_u8(100, 255, t),
                lerp_u8(130, 240, t),
                lerp_u8(180, 180, t),
            );
            shapes::fill_rect(fb, slot_x, slot_y - 1, slot_w, 1, hi_color);
            shapes::fill_rect(fb, slot_x, slot_y + slot_h, slot_w, 1, hi_color);
        }

        // ---------------------------------------------------------------
        // Sparkle particles — anchored at (cx, slot_y)
        // ---------------------------------------------------------------
        for p in &self.particles.particles {
            if !p.alive() {
                continue;
            }
            let alpha = p.life;
            let r = (p.color.r as f32 * alpha) as u8;
            let g = (p.color.g as f32 * alpha) as u8;
            let b = (p.color.b as f32 * alpha) as u8;
            let pc = Color::rgb(r, g, b);
            // Particle x/y are in units (fractions of size); anchor at slot center.
            let px = (p.x * size as f32) as i32 + cx;
            let py = (p.y * size as f32) as i32 + slot_y;
            let sz = ((p.size * (0.5 + 0.5 * p.life)) as i32).max(1);
            shapes::fill_rect(fb, px - sz / 2, py - sz / 2, sz, sz, pc);
        }
    }

    fn bounds_overflow(&self) -> i32 {
        // Sparkles can travel ~24 px outside the icon bounds.
        24
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.lift_y.impulse(0.0, 0.0);
        self.lift_y.set_target(0.0);
        self.insert_y.impulse(0.0, 0.0);
        self.insert_y.set_target(0.0);
        self.slot_glow = 0.0;
        self.click_time = 0;
        self.ejecting = false;
        self.sparkles_emitted = false;
        self.particles.clear();
    }

    fn name(&self) -> &'static str {
        "Save"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline(always)]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.max(0.0).min(1.0);
    (a as f32 + (b as f32 - a as f32) * t) as u8
}
