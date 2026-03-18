//! Easing functions for smooth animation curves.
//!
//! All functions take `t` in 0.0..=1.0 and return a value in approximately
//! the same range (some overshoot slightly for springy/back effects).
//!
//! No libm dependency — uses polynomial and parabolic approximations.

/// Linear (no easing).
pub fn linear(t: f32) -> f32 {
    t
}

/// Ease out cubic — fast start, smooth stop. Good for settle animations.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t1 = 1.0 - t;
    1.0 - t1 * t1 * t1
}

/// Ease in cubic — slow start, fast end.
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

/// Ease in-out cubic — slow at both ends, fast in the middle.
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let t2 = -2.0 * t + 2.0;
        1.0 - t2 * t2 * t2 / 2.0
    }
}

/// Elastic ease out — overshoots then settles (springy feel).
///
/// Uses a sine approximation; no libm required.
pub fn ease_out_elastic(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    // c4 = TAU / 3
    let c4 = 2.0 * 3.14159265 / 3.0;
    let decay = exp_approx(-10.0 * t);
    let wave = sin_approx((t * 10.0 - 0.75) * c4);
    decay * wave + 1.0
}

/// Bounce ease out — bounces like a dropped ball.
pub fn ease_out_bounce(t: f32) -> f32 {
    let n1: f32 = 7.5625;
    let d1: f32 = 2.75;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        let t = t - 1.5 / d1;
        n1 * t * t + 0.75
    } else if t < 2.5 / d1 {
        let t = t - 2.25 / d1;
        n1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / d1;
        n1 * t * t + 0.984375
    }
}

/// Back ease out — overshoots slightly then returns (anticipation feel).
pub fn ease_out_back(t: f32) -> f32 {
    let c1: f32 = 1.70158;
    let c3 = c1 + 1.0;
    let t1 = t - 1.0;
    1.0 + c3 * t1 * t1 * t1 + c1 * t1 * t1
}

/// Interpolate between two f32 values.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolate between two i32 values.
#[inline]
pub fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t) as i32
}

// ---------------------------------------------------------------------------
// Internal math approximations (no libm, no std)
// ---------------------------------------------------------------------------

/// Sine approximation using a parabolic method.
/// Accurate to within ~1% for smooth animations.
pub fn sin_approx(x: f32) -> f32 {
    const PI: f32 = 3.14159265;
    const TAU: f32 = 6.28318530;

    // Normalize to -PI..PI
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }

    // Parabolic approximation: 4x/pi - 4x²/pi² (with refinement)
    let abs_x = if x < 0.0 { -x } else { x };
    let y = x * (4.0 / PI - 4.0 / (PI * PI) * abs_x);
    // Refinement: 0.225 * (y * |y| - y) + y  (Bhaskara I refinement)
    0.225 * (y * (if y < 0.0 { -y } else { y }) - y) + y
}

/// Cosine approximation via phase shift of sin_approx.
pub fn cos_approx(x: f32) -> f32 {
    sin_approx(x + 3.14159265 / 2.0)
}

/// Approximate e^x using a piecewise polynomial.
/// Range clamped to avoid overflow; sufficient for animation decay curves.
fn exp_approx(x: f32) -> f32 {
    // For x in a reasonable range, use the identity e^x ≈ (1 + x/n)^n
    // with n=8 unrolled (fast for negative x like decay curves).
    // We clamp to avoid degenerate values.
    let x = x.max(-20.0).min(20.0);

    // Use the 2^x identity: e^x = 2^(x / ln2), approximate 2^f for f in 0..1
    // then use integer powers for the integer part.
    const LN2_INV: f32 = 1.4426950; // 1/ln(2)
    let t = x * LN2_INV;
    let int_part = t as i32;
    let frac = t - int_part as f32;

    // 2^frac approximation for frac in 0..1 (polynomial fit)
    let p2frac = 1.0 + frac * (0.6931472 + frac * (0.2402265 + frac * 0.0555041));

    // Scale by 2^int_part via repeated doubling/halving
    let scale = int_pow2(int_part);
    p2frac * scale
}

/// Compute 2^n for integer n (positive or negative).
fn int_pow2(n: i32) -> f32 {
    if n >= 0 {
        let n = n.min(126) as u32;
        // Build IEEE 754 float: exponent field = n + 127
        f32::from_bits((n + 127) << 23)
    } else {
        let n = (-n).min(126) as u32;
        f32::from_bits((127 - n) << 23)
    }
}
