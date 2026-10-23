//! Core icon trait and shared state machine.

use libgfx::surface::Surface as FrameBuf;

// Re-export Color so icon implementations don't need to import libgfx directly.
pub use libgfx::color::Color as IconColor;

/// Mouse state relative to an icon, supplied by the UI framework each frame.
#[derive(Clone, Copy, Debug)]
pub struct IconMouse {
    /// Is the cursor within the icon's hover zone?
    pub hovering: bool,
    /// Is a mouse button currently held down?
    pub pressed: bool,
    /// Was the mouse button just pressed this frame?
    pub just_clicked: bool,
    /// Was the mouse button just released this frame?
    pub just_released: bool,
    /// Mouse X relative to icon center, normalized –1.0 to 1.0.
    pub rel_x: f32,
    /// Mouse Y relative to icon center, normalized –1.0 to 1.0.
    pub rel_y: f32,
}

impl IconMouse {
    /// Construct a neutral mouse state (no interaction).
    pub fn none() -> Self {
        Self {
            hovering: false,
            pressed: false,
            just_clicked: false,
            just_released: false,
            rel_x: 0.0,
            rel_y: 0.0,
        }
    }
}

/// Animation state machine for icons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconState {
    /// Resting state — subtle idle animation (breathing, slow pulse, etc.).
    Idle,
    /// Mouse entered the hover zone — icon "notices" the cursor.
    HoverIn,
    /// Mouse is hovering — sustained hover animation.
    Hovering,
    /// Mouse left the hover zone — icon settles back to idle.
    HoverOut,
    /// Mouse button is held down — squash/compress animation.
    Pressed,
    /// Mouse button was just released — the payoff animation.
    Clicked,
}

/// Trait that every animated icon implements.
pub trait Icon {
    /// Advance animation by `dt_us` microseconds with the given mouse input.
    fn update(&mut self, dt_us: u32, mouse: IconMouse);

    /// Draw the icon centered at (cx, cy) with the given base size (width = height).
    fn draw(&self, fb: &mut FrameBuf, cx: i32, cy: i32, size: i32);

    /// Extra pixels beyond the base rect that animations may extend into.
    /// Used by layout to reserve overflow space around the icon.
    fn bounds_overflow(&self) -> i32;

    /// The icon's current animation state.
    fn state(&self) -> IconState;

    fn hover_amount(&self) -> f32;
    fn settled(&self) -> bool;

    /// Reset to idle, clearing all animation state.
    fn reset(&mut self);

    /// Human-readable name for this icon (e.g. "Home", "Back Arrow").
    fn name(&self) -> &'static str;
}

/// Shared base state used by all icon implementations.
///
/// Handles the state machine transitions so individual icons only need to
/// implement their visual responses to each state.
pub struct IconBase {
    pub state: IconState,
    /// Time spent in the current state, in microseconds.
    pub state_time: u32,
    /// Accumulated idle time for breathing / idle animations, in microseconds.
    pub idle_time: u32,
    /// Normalized transition progress 0.0..=1.0 for the current state.
    pub progress: f32,
    hover: f32,
    previous_hover: bool,
    previous_pressed: bool,
    remainder_us: u32,
    pending_click: bool,
    pending_release: bool,
}

impl IconBase {
    pub fn new() -> Self {
        Self {
            state: IconState::Idle,
            state_time: 0,
            idle_time: 0,
            progress: 0.0,
            previous_hover: false, previous_pressed: false,
            hover: 0.0, remainder_us: 0, pending_click: false, pending_release: false,
        }
    }

    /// Drive the state machine forward.
    ///
    /// Returns `true` if the state changed this frame.
    pub fn update(&mut self, dt_us: u32, mouse: &IconMouse) -> bool {
        let mut changed = false;
        let mut input = *mouse;
        self.remainder_us += dt_us % 1000;
        let quanta = dt_us/1000 + self.remainder_us/1000;
        self.remainder_us %= 1000;
        self.pending_click |= mouse.just_clicked;
        self.pending_release |= mouse.just_released;
        for _ in 0..quanta {
            input.just_clicked = self.pending_click;
            input.just_released = self.pending_release;
            self.pending_click = false; self.pending_release = false;
            changed |= self.tick(&input);
        }
        changed
    }
    pub fn hover_amount(&self) -> f32 { self.hover }
    pub fn settled(&self) -> bool {
        matches!(self.state,IconState::Idle | IconState::Hovering | IconState::Pressed)
            && (self.hover == 0.0 || self.hover == 1.0)
    }
    fn tick(&mut self, mouse: &IconMouse) -> bool {
        let dt_us = 1000;
        self.state_time = self.state_time.saturating_add(dt_us);
        // Restart finite idle effects on input, not on internal timed transitions.
        if mouse.hovering != self.previous_hover || mouse.pressed != self.previous_pressed
            || mouse.just_clicked || mouse.just_released {
            self.idle_time = 0;
        }
        self.previous_hover = mouse.hovering;
        self.previous_pressed = mouse.pressed;
        // Idle effects reach a stable phase after input settles.
        self.idle_time = self.idle_time.saturating_add(dt_us).min(2_000_000);
        if mouse.hovering { self.hover = (self.hover + 0.004).min(1.0); }
        else { self.hover = (self.hover - 0.005).max(0.0); }

        let old_state = self.state;

        match self.state {
            IconState::Idle => {
                if mouse.hovering {
                    self.transition(IconState::HoverIn);
                }
            }

            IconState::HoverIn => {
                // HoverIn lasts ~250 ms, then becomes Hovering.
                self.progress = (self.state_time as f32 / 250_000.0).min(1.0);
                if mouse.just_clicked {
                    self.transition(IconState::Pressed);
                } else if !mouse.hovering {
                    self.transition(IconState::HoverOut);
                } else if self.state_time >= 250_000 {
                    self.transition(IconState::Hovering);
                }
            }

            IconState::Hovering => {
                if mouse.just_clicked {
                    self.transition(IconState::Pressed);
                } else if !mouse.hovering {
                    self.transition(IconState::HoverOut);
                }
            }

            IconState::HoverOut => {
                // HoverOut lasts ~200 ms, then becomes Idle.
                self.progress = (self.state_time as f32 / 200_000.0).min(1.0);
                if mouse.hovering {
                    self.transition(IconState::HoverIn);
                } else if self.state_time >= 200_000 {
                    self.transition(IconState::Idle);
                }
            }

            IconState::Pressed => {
                // Held until release; compress animates over ~50 ms.
                self.progress = (self.state_time as f32 / 50_000.0).min(1.0);
                if mouse.just_released {
                    self.transition(IconState::Clicked);
                } else if !mouse.hovering && !mouse.pressed {
                    self.transition(IconState::HoverOut);
                }
            }

            IconState::Clicked => {
                // Payoff animation lasts ~500 ms.
                self.progress = (self.state_time as f32 / 500_000.0).min(1.0);
                if self.state_time >= 500_000 {
                    if mouse.hovering {
                        self.transition(IconState::Hovering);
                    } else {
                        self.transition(IconState::Idle);
                    }
                }
            }
        }

        old_state != self.state
    }

    fn transition(&mut self, new_state: IconState) {
        self.state = new_state;
        self.state_time = 0;
        self.progress = 0.0;
    }

    pub fn reset(&mut self) {
        self.state = IconState::Idle;
        self.state_time = 0;
        self.idle_time = 0;
        self.progress = 0.0;
        self.previous_hover = false; self.previous_pressed = false;
        self.hover = 0.0; self.remainder_us = 0; self.pending_click = false; self.pending_release = false;
    }
}
