//! Core icon trait and shared state machine.

use libgfx::framebuf::FrameBuf;

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
}

impl IconBase {
    pub fn new() -> Self {
        Self {
            state: IconState::Idle,
            state_time: 0,
            idle_time: 0,
            progress: 0.0,
        }
    }

    /// Drive the state machine forward.
    ///
    /// Returns `true` if the state changed this frame.
    pub fn update(&mut self, dt_us: u32, mouse: &IconMouse) -> bool {
        self.state_time += dt_us;
        self.idle_time += dt_us;

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
    }
}
