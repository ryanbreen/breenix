/// Snapshot of mouse input state with edge detection.
#[derive(Clone, Copy, Debug)]
pub struct InputState {
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Left button currently held.
    pub mouse_down: bool,
    /// Left button just pressed (rising edge).
    pub mouse_pressed: bool,
    /// Left button just released (falling edge).
    pub mouse_released: bool,
}

impl InputState {
    /// Construct from raw mouse_state() return values.
    ///
    /// `buttons` and `prev_buttons` are button bitmasks where bit 0 = left button.
    pub fn from_raw(mx: i32, my: i32, buttons: u32, prev_buttons: u32) -> Self {
        let down = (buttons & 1) != 0;
        let was_down = (prev_buttons & 1) != 0;
        Self {
            mouse_x: mx,
            mouse_y: my,
            mouse_down: down,
            mouse_pressed: down && !was_down,
            mouse_released: !down && was_down,
        }
    }
}

/// Events returned by widget update methods.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidgetEvent {
    /// No interaction occurred.
    None,
    /// Widget was clicked.
    Clicked,
    /// Checkbox or toggle changed state.
    Toggled(bool),
    /// Slider or numeric input value changed.
    ValueChanged(f32),
}
