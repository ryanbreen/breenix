
pub use writers::{ScreenChar, Color, ColorCode};

pub const BUFFER_HEIGHT: usize = 25;
pub const BUFFER_WIDTH: usize = 80;

pub const GREEN_BLANK: ScreenChar = ScreenChar {
    ascii_character: b'-',
    color_code: ColorCode::new(Color::LightGreen, Color::Black),
};

pub const RED_BLANK: ScreenChar = ScreenChar {
    ascii_character: b' ',
    color_code: ColorCode::new(Color::LightRed, Color::Black),
};

pub const GRAY_BLANK: ScreenChar = ScreenChar {
    ascii_character: b' ',
    color_code: ColorCode::new(Color::LightGray, Color::Black),
};
