use libgfx::color::Color;

/// Visual theme for widget rendering.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub widget_bg: Color,
    pub widget_bg_hover: Color,
    pub widget_bg_active: Color,
    pub accent: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub border: Color,
    pub panel_bg: Color,
    pub padding: i32,
    pub spacing: i32,
    pub border_width: i32,
    /// If true, use the anti-aliased Noto Sans Mono 16px font.
    /// If false, use the 5x7 bitmap font at scale=1.
    pub use_bitmap_font: bool,
}

impl Theme {
    /// Dark theme matching guskit's existing color palette.
    pub fn dark() -> Self {
        Self {
            widget_bg: Color::rgb(60, 60, 60),
            widget_bg_hover: Color::rgb(80, 80, 80),
            widget_bg_active: Color::rgb(100, 140, 220),
            accent: Color::rgb(70, 130, 220),
            text_primary: Color::WHITE,
            text_secondary: Color::rgb(180, 180, 180),
            border: Color::rgb(120, 120, 120),
            panel_bg: Color::rgb(45, 45, 45),
            padding: 4,
            spacing: 4,
            border_width: 1,
            use_bitmap_font: false,
        }
    }

    /// Light theme for bright UI contexts.
    pub fn light() -> Self {
        Self {
            widget_bg: Color::rgb(220, 220, 220),
            widget_bg_hover: Color::rgb(200, 200, 200),
            widget_bg_active: Color::rgb(100, 140, 220),
            accent: Color::rgb(50, 110, 200),
            text_primary: Color::BLACK,
            text_secondary: Color::rgb(80, 80, 80),
            border: Color::rgb(160, 160, 160),
            panel_bg: Color::rgb(240, 240, 240),
            padding: 4,
            spacing: 4,
            border_width: 1,
            use_bitmap_font: false,
        }
    }
}
