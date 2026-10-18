//! Breenix's geometric U+2500..U+257F glyphs. An arm is absent, light,
//! heavy or double; coordinates are derived from the current cell dimensions.
//! This table is the glyph geometry, shared with the pixel read-back oracle.

#[derive(Clone, Copy)]
struct Arms {
    up: u8,
    right: u8,
    down: u8,
    left: u8,
}

const fn arms(up: u8, right: u8, down: u8, left: u8) -> Arms {
    Arms {
        up,
        right,
        down,
        left,
    }
}

const SHAPES: [Arms; 128] = [
    arms(0, 1, 0, 1),
    arms(0, 2, 0, 2),
    arms(1, 0, 1, 0),
    arms(2, 0, 2, 0),
    arms(0, 1, 0, 1),
    arms(0, 2, 0, 2),
    arms(1, 0, 1, 0),
    arms(2, 0, 2, 0),
    arms(0, 1, 0, 1),
    arms(0, 2, 0, 2),
    arms(1, 0, 1, 0),
    arms(2, 0, 2, 0),
    arms(0, 1, 1, 0),
    arms(0, 2, 1, 0),
    arms(0, 1, 2, 0),
    arms(0, 2, 2, 0),
    arms(0, 0, 1, 1),
    arms(0, 0, 1, 2),
    arms(0, 0, 2, 1),
    arms(0, 0, 2, 2),
    arms(1, 1, 0, 0),
    arms(1, 2, 0, 0),
    arms(2, 1, 0, 0),
    arms(2, 2, 0, 0),
    arms(1, 0, 0, 1),
    arms(1, 0, 0, 2),
    arms(2, 0, 0, 1),
    arms(2, 0, 0, 2),
    arms(1, 1, 1, 0),
    arms(1, 2, 1, 0),
    arms(2, 1, 1, 0),
    arms(1, 1, 2, 0),
    arms(2, 1, 2, 0),
    arms(2, 2, 1, 0),
    arms(1, 2, 2, 0),
    arms(2, 2, 2, 0),
    arms(1, 0, 1, 1),
    arms(1, 0, 1, 2),
    arms(2, 0, 1, 1),
    arms(1, 0, 2, 1),
    arms(2, 0, 2, 1),
    arms(2, 0, 1, 2),
    arms(1, 0, 2, 2),
    arms(2, 0, 2, 2),
    arms(0, 1, 1, 1),
    arms(0, 1, 1, 2),
    arms(0, 2, 1, 1),
    arms(0, 2, 1, 2),
    arms(0, 1, 2, 1),
    arms(0, 1, 2, 2),
    arms(0, 2, 2, 1),
    arms(0, 2, 2, 2),
    arms(1, 1, 0, 1),
    arms(1, 1, 0, 2),
    arms(1, 2, 0, 1),
    arms(1, 2, 0, 2),
    arms(2, 1, 0, 1),
    arms(2, 1, 0, 2),
    arms(2, 2, 0, 1),
    arms(2, 2, 0, 2),
    arms(1, 1, 1, 1),
    arms(1, 1, 1, 2),
    arms(1, 2, 1, 1),
    arms(1, 2, 1, 2),
    arms(2, 1, 1, 1),
    arms(1, 1, 2, 1),
    arms(2, 1, 2, 1),
    arms(2, 1, 1, 2),
    arms(2, 2, 1, 1),
    arms(1, 1, 2, 2),
    arms(1, 2, 2, 1),
    arms(2, 2, 1, 2),
    arms(1, 2, 2, 2),
    arms(2, 1, 2, 2),
    arms(2, 2, 2, 1),
    arms(2, 2, 2, 2),
    arms(0, 1, 0, 1),
    arms(0, 2, 0, 2),
    arms(1, 0, 1, 0),
    arms(2, 0, 2, 0),
    arms(0, 3, 0, 3),
    arms(3, 0, 3, 0),
    arms(0, 3, 1, 0),
    arms(0, 1, 3, 0),
    arms(0, 3, 3, 0),
    arms(0, 0, 1, 3),
    arms(0, 0, 3, 1),
    arms(0, 0, 3, 3),
    arms(1, 3, 0, 0),
    arms(3, 1, 0, 0),
    arms(3, 3, 0, 0),
    arms(1, 0, 0, 3),
    arms(3, 0, 0, 1),
    arms(3, 0, 0, 3),
    arms(1, 3, 1, 0),
    arms(3, 1, 3, 0),
    arms(3, 3, 3, 0),
    arms(1, 0, 1, 3),
    arms(3, 0, 3, 1),
    arms(3, 0, 3, 3),
    arms(0, 3, 1, 3),
    arms(0, 1, 3, 1),
    arms(0, 3, 3, 3),
    arms(1, 3, 0, 3),
    arms(3, 1, 0, 1),
    arms(3, 3, 0, 3),
    arms(1, 3, 1, 3),
    arms(3, 1, 3, 1),
    arms(3, 3, 3, 3),
    arms(0, 1, 1, 0),
    arms(0, 0, 1, 1),
    arms(1, 0, 0, 1),
    arms(1, 1, 0, 0),
    arms(0, 0, 0, 0),
    arms(0, 0, 0, 0),
    arms(0, 0, 0, 0),
    arms(0, 0, 0, 1),
    arms(1, 0, 0, 0),
    arms(0, 1, 0, 0),
    arms(0, 0, 1, 0),
    arms(0, 0, 0, 2),
    arms(2, 0, 0, 0),
    arms(0, 2, 0, 0),
    arms(0, 0, 2, 0),
    arms(0, 2, 0, 1),
    arms(1, 0, 2, 0),
    arms(0, 1, 0, 2),
    arms(2, 0, 1, 0),
];

fn band(position: i32, centre: i32, thickness: i32) -> bool {
    position >= centre - (thickness - 1) / 2 && position <= centre + thickness / 2
}

/// Integer coverage, including the cursor band so adjacent text rows join.
pub fn coverage(glyph: u16, width: usize, height: usize, x: usize, y: usize) -> u8 {
    if !(0x2500..=0x257f).contains(&glyph) || x >= width || y >= height {
        return 0;
    }
    let index = usize::from(glyph - 0x2500);
    let (x, y, w, h) = (x as i32, y as i32, width as i32, height as i32);
    let (cx, cy) = (w / 2, h / 2);
    let light = (h / 16).max(1);
    let shape = SHAPES[index];
    if (0x71..=0x73).contains(&index) {
        let diagonal = |rising: bool| {
            let yy = if rising { h - 1 - y } else { y };
            (x * (h - 1) - yy * (w - 1)).abs() <= light * (h - 1) / 2
        };
        return if (index != 0x72 && diagonal(true)) || (index != 0x71 && diagonal(false)) {
            255
        } else {
            0
        };
    }
    // Rounded corners replace the central right angle by a quarter annulus.
    if (0x6d..=0x70).contains(&index) {
        let radius = (w / 3).max(2);
        let sx = if shape.right != 0 { 1 } else { -1 };
        let sy = if shape.down != 0 { 1 } else { -1 };
        let dx = (x - cx) * sx;
        let dy = (y - cy) * sy;
        let ink = if dx >= radius {
            band(y, cy, light)
        } else if dy >= radius {
            band(x, cx, light)
        } else if dx >= 0 && dy >= 0 {
            let distance = (dx - radius) * (dx - radius) + (dy - radius) * (dy - radius);
            let outer = radius + light / 2;
            let inner = radius - (light + 1) / 2;
            distance <= outer * outer && distance >= inner * inner
        } else {
            false
        };
        return if ink { 255 } else { 0 };
    }
    let dashed = match index {
        4..=7 => 3,
        8..=11 => 4,
        0x4c..=0x4f => 2,
        _ => 0,
    };
    let horizontal = shape.right != 0 || shape.left != 0;
    if dashed != 0 {
        let (position, length) = if horizontal { (x, w) } else { (y, h) };
        // Each dash has a one-light-stroke gap, including the cell seam.
        if (position * dashed) % length >= length - (light * dashed).min(length / 2) {
            return 0;
        }
    }
    let stroke = |weight: u8, position: i32, centre: i32| match weight {
        1 => band(position, centre, light),
        2 => band(position, centre, 2 * light),
        3 => band(position, centre - light - 1, light) || band(position, centre + light + 1, light),
        _ => false,
    };
    let offset = light + 1;
    // Extend double corners to the corresponding perpendicular lane, leaving
    // the gap between the inner and outer strokes open.
    let right_start = if shape.right == 3 && shape.up == 3 && shape.down == 0 {
        cx - (y - cy).signum() * offset
    } else if shape.right == 3 && shape.down == 3 && shape.up == 0 {
        cx + (y - cy).signum() * offset
    } else if shape.up == 3 || shape.down == 3 {
        cx - offset
    } else {
        cx
    };
    let left_end = if shape.left == 3 && shape.up == 3 && shape.down == 0 {
        cx + (y - cy).signum() * offset
    } else if shape.left == 3 && shape.down == 3 && shape.up == 0 {
        cx - (y - cy).signum() * offset
    } else if shape.up == 3 || shape.down == 3 {
        cx + offset
    } else {
        cx
    };
    let down_start = if shape.down == 3 && shape.right == 3 && shape.left == 0 {
        cy + (x - cx).signum() * offset
    } else if shape.down == 3 && shape.left == 3 && shape.right == 0 {
        cy - (x - cx).signum() * offset
    } else if shape.right == 3 || shape.left == 3 {
        cy - offset
    } else {
        cy
    };
    let up_end = if shape.up == 3 && shape.right == 3 && shape.left == 0 {
        cy - (x - cx).signum() * offset
    } else if shape.up == 3 && shape.left == 3 && shape.right == 0 {
        cy + (x - cx).signum() * offset
    } else if shape.right == 3 || shape.left == 3 {
        cy + offset
    } else {
        cy
    };
    let ink = (x >= right_start && stroke(shape.right, y, cy))
        || (x <= left_end && stroke(shape.left, y, cy))
        || (y >= down_start && stroke(shape.down, x, cx))
        || (y <= up_end && stroke(shape.up, x, cx));
    if ink {
        255
    } else {
        0
    }
}
