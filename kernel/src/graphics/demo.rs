//! Graphics demonstration module.
//!
//! Provides a visual demo of the graphics stack capabilities
//! that runs during kernel boot.

use super::font::Font;
use super::primitives::{
    draw_circle, draw_line, draw_rect, draw_text, fill_circle, fill_rect, Canvas, Color, Rect,
    TextStyle,
};

/// Run a graphics demonstration on the given canvas.
///
/// This draws various shapes and text to showcase the graphics stack.
pub fn run_demo(canvas: &mut impl Canvas) {
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;

    // Clear to a dark blue background
    fill_rect(
        canvas,
        Rect {
            x: 0,
            y: 0,
            width: width as u32,
            height: height as u32,
        },
        Color::rgb(20, 30, 50),
    );

    // Draw a header banner
    fill_rect(
        canvas,
        Rect {
            x: 0,
            y: 0,
            width: width as u32,
            height: 60,
        },
        Color::rgb(40, 80, 120),
    );

    // Title text
    let title_style = TextStyle::new()
        .with_color(Color::WHITE)
        .with_font(Font::default_font());

    draw_text(canvas, 20, 20, "Breenix Graphics Stack Demo", &title_style);

    // Draw colorful rectangles
    let colors = [
        Color::RED,
        Color::GREEN,
        Color::BLUE,
        Color::rgb(255, 255, 0),   // Yellow
        Color::rgb(255, 0, 255),   // Magenta
        Color::rgb(0, 255, 255),   // Cyan
    ];

    let box_width = 80;
    let box_height = 60;
    let start_x = 50;
    let start_y = 100;

    for (i, &color) in colors.iter().enumerate() {
        let x = start_x + (i as i32 % 3) * (box_width + 20);
        let y = start_y + (i as i32 / 3) * (box_height + 20);

        // Filled rectangle
        fill_rect(
            canvas,
            Rect {
                x,
                y,
                width: box_width as u32,
                height: box_height as u32,
            },
            color,
        );

        // White border
        draw_rect(
            canvas,
            Rect {
                x: x - 2,
                y: y - 2,
                width: (box_width + 4) as u32,
                height: (box_height + 4) as u32,
            },
            Color::WHITE,
        );
    }

    // Draw circles section
    let circle_y = start_y + 180;
    let circle_text_style = TextStyle::new().with_color(Color::rgb(200, 200, 200));

    draw_text(canvas, 50, circle_y, "Circles:", &circle_text_style);

    // Filled circles
    fill_circle(canvas, 100, circle_y + 60, 30, Color::rgb(255, 100, 100));
    fill_circle(canvas, 180, circle_y + 60, 25, Color::rgb(100, 255, 100));
    fill_circle(canvas, 250, circle_y + 60, 20, Color::rgb(100, 100, 255));

    // Circle outlines
    draw_circle(canvas, 340, circle_y + 60, 35, Color::WHITE);
    draw_circle(canvas, 340, circle_y + 60, 25, Color::rgb(255, 200, 0));
    draw_circle(canvas, 340, circle_y + 60, 15, Color::rgb(255, 100, 0));

    // Draw lines section
    let lines_y = circle_y + 130;
    draw_text(canvas, 50, lines_y, "Lines:", &circle_text_style);

    // Draw radiating lines (pre-computed approximate directions)
    // Using 12 directions at 30-degree increments
    let center_x = 150;
    let center_y = lines_y + 60;
    let radius = 50i32;

    // Pre-computed (cos, sin) * 100 for angles 0, 30, 60, ..., 330 degrees
    let directions: [(i32, i32); 12] = [
        (100, 0),    // 0°
        (87, 50),    // 30°
        (50, 87),    // 60°
        (0, 100),    // 90°
        (-50, 87),   // 120°
        (-87, 50),   // 150°
        (-100, 0),   // 180°
        (-87, -50),  // 210°
        (-50, -87),  // 240°
        (0, -100),   // 270°
        (50, -87),   // 300°
        (87, -50),   // 330°
    ];

    for (i, (dx, dy)) in directions.iter().enumerate() {
        let end_x = center_x + (radius * dx) / 100;
        let end_y = center_y + (radius * dy) / 100;
        let intensity = ((i as u32 * 255) / 12) as u8;
        let color = Color::rgb(255, intensity, 255 - intensity);
        draw_line(canvas, center_x, center_y, end_x, end_y, color);
    }

    // Draw diagonal lines
    for i in 0..10 {
        let x1 = 280 + i * 8;
        let color = Color::rgb(50 + i as u8 * 20, 100 + i as u8 * 15, 200);
        draw_line(canvas, x1, lines_y + 20, x1 + 60, lines_y + 100, color);
    }

    // Text rendering showcase
    let text_y = lines_y + 140;
    draw_text(canvas, 50, text_y, "Text Rendering:", &circle_text_style);

    // Different colored text
    let red_style = TextStyle::new().with_color(Color::RED);
    let green_style = TextStyle::new().with_color(Color::GREEN);
    let blue_style = TextStyle::new().with_color(Color::BLUE);
    let yellow_style = TextStyle::new().with_color(Color::rgb(255, 255, 0));

    draw_text(canvas, 50, text_y + 30, "Red Text", &red_style);
    draw_text(canvas, 150, text_y + 30, "Green Text", &green_style);
    draw_text(canvas, 270, text_y + 30, "Blue Text", &blue_style);

    // Text with background
    let bg_style = TextStyle::new()
        .with_color(Color::BLACK)
        .with_background(Color::rgb(255, 255, 0));

    draw_text(canvas, 50, text_y + 60, " Highlighted Text ", &bg_style);

    // Multi-line text
    let multiline_style = TextStyle::new().with_color(Color::rgb(180, 180, 255));
    draw_text(
        canvas,
        50,
        text_y + 95,
        "Multi-line text:\n  Line 1\n  Line 2\n  Line 3",
        &multiline_style,
    );

    // Footer
    let footer_style = TextStyle::new().with_color(Color::rgb(100, 100, 100));
    draw_text(
        canvas,
        50,
        height - 40,
        "Phase 4: Text Rendering Complete!",
        &footer_style,
    );

    // Draw a decorative border
    draw_rect(
        canvas,
        Rect {
            x: 10,
            y: 70,
            width: (width - 20) as u32,
            height: (height - 120) as u32,
        },
        Color::rgb(60, 100, 140),
    );
}
