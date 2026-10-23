pub fn run() {
    use libicon::particles::{Particle, ParticlePool};
    use libgfx::color::Color;
    for edge in [0.0, -0.25, -3.0] {
        let mut pool = ParticlePool::with_bounds(1, edge - 4.0, edge - 4.0, edge, edge);
        pool.emit(Particle { x: edge, y: edge, vx: 0.0, vy: 0.0, life: 1.0,
            max_life: 1.0, size: 1.0, color: Color::WHITE, gravity: 0.0, friction: 0.0 });
        let mut frame = crate::raster::Drawn::bgrx(20,20);
        frame.reset(Color::BLACK);
        pool.draw(&mut frame.fb(),10,10);
        let mut ink = 0;
        for y in 0..20 { for x in 0..20 {
            if frame.color(x,y) != Color::BLACK {
                ink += 1;
                assert!((x as f32) >= edge + 6.0 && x as f32 + 1.0 <= edge + 10.0);
                assert!((y as f32) >= edge + 6.0 && y as f32 + 1.0 <= edge + 10.0);
            }
        }}
        assert_eq!(ink, 1, "a one-pixel particle must fit at edge {edge}");
    }
}
