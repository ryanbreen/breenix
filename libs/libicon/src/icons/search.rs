//! Search icon — magnifying glass with shimmer and sonar-ring click effect.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::{cos_approx, sin_approx};
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::physics::Spring;

pub struct SearchIcon {
    base: IconBase,
    /// Overall lens scale.
    scale: Spring,
    /// Angle (radians) of the orbiting shimmer highlight.
    shimmer_angle: f32,
    /// Sonar ring state: up to 3 rings, each with (birth_time_us, is_alive).
    sonar_rings: [(u32, bool); 3],
    /// Time elapsed since Clicked state started (us).
    click_time: u32,
}

impl SearchIcon {
    pub fn new() -> Self {
        Self {
            base: IconBase::new(),
            scale: Spring::new(1.0, 280.0, 18.0),
            shimmer_angle: 0.0,
            sonar_rings: [(0, false); 3],
            click_time: 0,
        }
    }

    fn emit_sonar_burst(&mut self) {
        // Spawn 3 rings with staggered birth times tracked in sonar_rings.
        self.sonar_rings[0] = (0, true);
        self.sonar_rings[1] = (120_000, true);
        self.sonar_rings[2] = (240_000, true);
        self.click_time = 0;
    }
}

impl Icon for SearchIcon {
    fn update(&mut self, dt_us: u32, mouse: IconMouse) {
        let state_changed = self.base.update(dt_us, &mouse);
        let dt = dt_us as f32 / 1_000_000.0;

        // Trigger sonar on click.
        if state_changed && self.base.state == IconState::Clicked {
            self.emit_sonar_burst();
        }

        // Advance click timer.
        if self.base.state == IconState::Clicked {
            self.click_time += dt_us;
        }

        // Advance shimmer angle when hovering.
        if matches!(
            self.base.state,
            IconState::HoverIn | IconState::Hovering | IconState::Clicked
        ) {
            // Full orbit in ~2 seconds.
            self.shimmer_angle += dt * 3.14159265;
        }

        // Scale targets.
        let target_scale = match self.base.state {
            IconState::Pressed => 0.88,
            IconState::HoverIn | IconState::Hovering => 1.12,
            IconState::Clicked => 1.15,
            _ => 1.0,
        };
        self.scale.set_target(target_scale);
        self.scale.update(dt);
    }

    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32) {
        let sc = self.scale.value;
        // Idle pulse: lens slightly breathes.
        let breathe = 1.0 + sin_approx(self.base.idle_time as f32 / 1_000_000.0 * 1.8) * 0.015;
        let s = (size as f32 * sc * breathe) as i32;
        if s < 4 {
            return;
        }

        // Lens centre is offset slightly up-left; handle goes bottom-right.
        let lens_cx = cx - s / 8;
        let lens_cy = cy - s / 8;
        let radius = s * 35 / 100;

        // Draw lens (circle outline, glass blue).
        let lens_color = Color::rgb(180, 210, 240);
        shapes::draw_circle(fb, lens_cx, lens_cy, radius, lens_color);
        // Inner highlight ring.
        if radius > 4 {
            shapes::draw_circle(fb, lens_cx, lens_cy, radius - 2, Color::rgb(140, 180, 220));
        }

        // Handle (diagonal line, bottom-right of lens).
        let handle_start_x = lens_cx + radius * 7 / 10;
        let handle_start_y = lens_cy + radius * 7 / 10;
        let handle_end_x = cx + s * 42 / 100;
        let handle_end_y = cy + s * 42 / 100;
        let handle_color = Color::rgb(160, 140, 120);
        shapes::draw_line(fb, handle_start_x, handle_start_y, handle_end_x, handle_end_y, handle_color);
        // Handle thickness: second parallel line.
        shapes::draw_line(
            fb,
            handle_start_x + 1,
            handle_start_y + 1,
            handle_end_x + 1,
            handle_end_y + 1,
            handle_color,
        );

        // Shimmer highlight: a bright 2×2 dot orbiting inside the lens.
        if matches!(
            self.base.state,
            IconState::HoverIn | IconState::Hovering | IconState::Clicked
        ) {
            let orbit_r = (radius as f32 * 0.5) as i32;
            let sh_x = lens_cx + (cos_approx(self.shimmer_angle) * orbit_r as f32) as i32;
            let sh_y = lens_cy + (sin_approx(self.shimmer_angle) * orbit_r as f32) as i32;
            shapes::fill_rect(fb, sh_x, sh_y, 2, 2, Color::rgb(255, 255, 255));
        }

        // Sonar rings: expanding circles with fading colour.
        // Each ring born at `birth_ms` after click, expands for 400ms.
        if self.base.state == IconState::Clicked {
            let elapsed = self.click_time;
            for &(birth_ms, alive) in &self.sonar_rings {
                if !alive {
                    continue;
                }
                if elapsed < birth_ms {
                    continue;
                }
                let ring_age_ms = elapsed - birth_ms;
                if ring_age_ms > 450_000 {
                    continue;
                }
                let t = ring_age_ms as f32 / 450_000.0;
                // Expand from radius to radius + 60px.
                let ring_r = radius + (t * 60.0) as i32;
                // Fade from bright cyan to transparent.
                let intensity = ((1.0 - t) * 200.0) as u8;
                let ring_color = Color::rgb(
                    (intensity as u32 * 80 / 200) as u8,
                    intensity,
                    intensity,
                );
                shapes::draw_circle(fb, lens_cx, lens_cy, ring_r, ring_color);
            }
        }

        // Also draw lingering rings during HoverOut right after click.
        // (They fade naturally as click_time advances.)
    }

    fn bounds_overflow(&self) -> i32 {
        // Sonar rings expand up to 60px beyond icon.
        64
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.shimmer_angle = 0.0;
        self.sonar_rings = [(0, false); 3];
        self.click_time = 0;
    }

    fn name(&self) -> &'static str {
        "Search"
    }
}
