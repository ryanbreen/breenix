//! Spring physics for smooth, organic animation.
//!
//! Critically-damped (or under-damped) springs let values chase their targets
//! with natural momentum and optional overshoot.

/// A spring that tracks a scalar value toward a target.
///
/// Update with `update(dt_seconds)` each frame.
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    /// Stiffness (higher = snappier). Good range: 100–500.
    pub stiffness: f32,
    /// Damping (higher = less bouncy). Good range: 10–30.
    pub damping: f32,
}

impl Spring {
    /// Create a spring at `initial` position with given stiffness and damping.
    pub fn new(initial: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
            stiffness,
            damping,
        }
    }

    /// Advance the spring by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        let force = -self.stiffness * (self.value - self.target);
        let damping_force = -self.damping * self.velocity;
        self.velocity += (force + damping_force) * dt;
        self.value += self.velocity * dt;
    }

    /// Returns true if the spring has settled close to its target.
    pub fn settled(&self) -> bool {
        let diff = self.value - self.target;
        diff * diff < 0.01 && self.velocity * self.velocity < 0.01
    }

    /// Set a new target for the spring to chase.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Instantly snap to a value and optionally add an impulse velocity.
    pub fn impulse(&mut self, value: f32, velocity: f32) {
        self.value = value;
        self.velocity = velocity;
    }
}

/// A 2D spring for position animation.
pub struct Spring2D {
    pub x: Spring,
    pub y: Spring,
}

impl Spring2D {
    /// Create a 2D spring at (x, y) with given stiffness and damping.
    pub fn new(x: f32, y: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            x: Spring::new(x, stiffness, damping),
            y: Spring::new(y, stiffness, damping),
        }
    }

    /// Advance both axes by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        self.x.update(dt);
        self.y.update(dt);
    }

    /// Set a new 2D target.
    pub fn set_target(&mut self, x: f32, y: f32) {
        self.x.set_target(x);
        self.y.set_target(y);
    }

    /// Returns true if both axes have settled.
    pub fn settled(&self) -> bool {
        self.x.settled() && self.y.settled()
    }
}
