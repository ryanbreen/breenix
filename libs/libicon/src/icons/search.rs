//! Search icon — magnifying glass with shimmer and sonar-ring click effect.

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::easing::{cos_approx, sin_approx};
use crate::icon::{Icon, IconBase, IconMouse, IconState};
use crate::physics::Spring;

pub struct SearchIcon {
    base: IconBase,
    remainder_us: u32,
    pending_click: bool,
    pending_release: bool,
    /// Overall lens scale.
    scale: Spring,
    /// Angle (radians) of the orbiting shimmer highlight.
    shimmer_angle: f32,
    shimmer_time: u32,
    /// Sonar ring state: up to 3 rings, each with (birth_time_us, is_alive).
    sonar_rings: [(u32, bool); 3],
    /// Time elapsed since Clicked state started (us).
    click_time: u32,
}

impl SearchIcon {
    pub fn new() -> Self {
        Self {
            base: IconBase::new(),
            remainder_us: 0,
            pending_click: false,
            pending_release: false,
            scale: Spring::new(1.0, 280.0, 18.0),
            shimmer_angle: 0.0,
            shimmer_time: 0,
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

impl SearchIcon {
    fn tick(&mut self, mouse: IconMouse) {
        let dt_us = 1000;
        let state_changed = self.base.update(dt_us, &mouse);
        let dt = dt_us as f32 / 1_000_000.0;

        // Trigger sonar on click.
        if state_changed && self.base.state == IconState::Clicked {
            self.emit_sonar_burst();
        }

        // Advance click timer.
        if self.sonar_rings.iter().any(|&(_, alive)| alive) {
            self.click_time = self.click_time.saturating_add(dt_us);
            for (birth, alive) in &mut self.sonar_rings {
                if self.click_time >= *birth + 450_000 {
                    *alive = false;
                }
            }
        }

        // Advance shimmer angle when hovering.
        if matches!(
            self.base.state,
            IconState::HoverIn | IconState::Hovering | IconState::Clicked
        ) {
            // Full orbit in ~2 seconds.
            self.shimmer_time = self.shimmer_time.saturating_add(dt_us).min(2_000_000);
            self.shimmer_angle = self.shimmer_time as f32 * (core::f32::consts::TAU / 2_000_000.0);
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
}

impl Icon for SearchIcon {
    fn update(&mut self, dt_us: u32, mut mouse: IconMouse) {
        self.remainder_us += dt_us % 1000;
        let quanta = dt_us / 1000 + self.remainder_us / 1000;
        self.remainder_us %= 1000;
        self.pending_click |= mouse.just_clicked;
        self.pending_release |= mouse.just_released;
        for _ in 0..quanta {
            mouse.just_clicked = self.pending_click;
            mouse.just_released = self.pending_release;
            self.pending_click = false;
            self.pending_release = false;
            self.tick(mouse);
        }
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
        shapes::draw_line(
            fb,
            handle_start_x,
            handle_start_y,
            handle_end_x,
            handle_end_y,
            handle_color,
        );
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
                let ring_color =
                    Color::rgb((intensity as u32 * 80 / 200) as u8, intensity, intensity);
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

    fn hover_amount(&self) -> f32 {
        self.base.hover_amount()
    }
    fn settled(&self) -> bool {
        self.base.settled()
            && self.base.idle_time >= 2_000_000
            && (!matches!(
                self.base.state,
                IconState::HoverIn | IconState::Hovering | IconState::Clicked
            ) || self.shimmer_time == 2_000_000)
            && self.scale.settled()
            && self.sonar_rings.iter().all(|&(_, alive)| !alive)
    }

    fn state(&self) -> IconState {
        self.base.state
    }

    fn reset(&mut self) {
        self.base.reset();
        self.remainder_us = 0;
        self.pending_click = false;
        self.pending_release = false;
        self.scale.impulse(1.0, 0.0);
        self.scale.set_target(1.0);
        self.shimmer_angle = 0.0;
        self.shimmer_time = 0;
        self.sonar_rings = [(0, false); 3];
        self.click_time = 0;
    }

    fn name(&self) -> &'static str {
        "Search"
    }
}
