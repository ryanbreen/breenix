//! GPU producer for the raw window fixture (breengel.md section 24).
//! The client owns no CPU image or replay shadow; verification reads the device.
use libbreenix::gpu;

#[derive(Clone, Copy)]
pub struct Target {
    pub handle: u64,
    pub width: u32,
    pub height: u32,
}
impl Target {
    pub fn read(self) -> Result<Vec<u8>, String> {
        let mut pixels = vec![0; (self.width * self.height * 4) as usize];
        let mut record = [0; gpu::abi::record::SIZE];
        let r = gpu::readback(
            self.handle,
            [0, 0, self.width, self.height],
            &mut pixels,
            self.width * 4,
            &mut record,
        );
        if r != 0 {
            return Err(format!("gpu-window: target readback returned {r}"));
        }
        Ok(pixels)
    }
    /// Fixture geometry is emitted as solid draws, never uploaded as a CPU image.
    pub fn paint(self, rect: [i32; 4], seed: u32) -> Result<(), String> {
        let mut context =
            bgl::Context::new().map_err(|r| format!("gpu-window: fixture context returned {r}"))?;
        let [x, y, w, h] = rect;
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = (
            (x as i64 + w as i64).min(self.width as i64),
            (y as i64 + h as i64).min(self.height as i64),
        );
        context.begin(self.handle, self.width, self.height);
        let mut draws = 0;
        for y in y0..y1 as i32 {
            for x in x0..x1 as i32 {
                let (x, y) = (x as u32, y as u32);
                let b = (7 * x + 13 * seed) & 255;
                let g = (5 * y + 29 * seed) & 255;
                let r = (x ^ y ^ seed) & 255;
                context.solid(
                    0xff000000 | r << 16 | g << 8 | b,
                    &[[x as i32, y as i32, x as i32 + 1, y as i32 + 1]],
                );
                draws += 1;
                if draws == 4096 {
                    context
                        .submit()
                        .and_then(|f| f.wait(2_000_000_000))
                        .map_err(|r| format!("gpu-window: fixture render returned {r}"))?;
                    context.begin(self.handle, self.width, self.height);
                    draws = 0;
                }
            }
        }
        if draws != 0 {
            context
                .submit()
                .and_then(|f| f.wait(2_000_000_000))
                .map_err(|r| format!("gpu-window: fixture render returned {r}"))?;
        }
        Ok(())
    }
}
