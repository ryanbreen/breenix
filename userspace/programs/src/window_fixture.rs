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
    /// Draw fixture geometry in batches, using the bottom row of a two-row gradient
    /// to select each instance's own colour. This issues geometry, with no pixel upload.
    pub fn paint(self, rect: [i32; 4], seed: u32) -> Result<(), String> {
        use std::cell::RefCell;
        thread_local! {
            static RENDER: RefCell<(u64, Option<bgl::Context>)> = const { RefCell::new((0, None)) };
        }
        let pid = libbreenix::process::getpid().map_err(|e| format!("fixture pid: {e}"))?.raw();
        RENDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.0 != pid {
                slot.1 = None;
                slot.0 = pid;
            }
            if slot.1.is_none() {
                slot.1 = Some(bgl::Context::new().map_err(|r| format!("gpu-window: fixture context returned {r}"))?);
            }
            let result = self.paint_with(slot.1.as_mut().expect("fixture context created"), rect, seed);
            if result.is_err() { slot.1 = None; }
            result
        })
    }

    fn paint_with(self, context: &mut bgl::Context, rect: [i32; 4], seed: u32) -> Result<(), String> {
        const BATCH: usize = 8192;
        let [x, y, w, h] = rect;
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = (
            (x as i64 + w as i64).min(self.width as i64),
            (y as i64 + h as i64).min(self.height as i64),
        );
        context.discard();
        context.begin(self.handle, self.width, self.height);
        let mut instances = Vec::with_capacity(BATCH);
        let flush = |context: &mut bgl::Context, instances: &mut Vec<bgl::Instance>| -> Result<(), String> {
            context.draw(bgl::pipeline::GRADIENT_V, 0xff000000, 0, 0, 0, instances);
            context.submit().and_then(|f| f.wait(2_000_000_000))
                .map_err(|r| format!("gpu-window: fixture render returned {r}"))?;
            instances.clear();
            Ok(())
        };
        for y in y0..y1 as i32 {
            for x in x0..x1 as i32 {
                let (ux, uy) = (x as u32, y as u32);
                let b = (7 * ux + 13 * seed) & 255;
                let g = (5 * uy + 29 * seed) & 255;
                let r = (ux ^ uy ^ seed) & 255;
                let mut params = [0; 16];
                params[..3].copy_from_slice(&[(y - 1) as u32, 2, 0xff000000 | r << 16 | g << 8 | b]);
                instances.push(bgl::Instance { rect: [x, y, x + 1, y + 1], params });
                if instances.len() == BATCH {
                    flush(context, &mut instances)?;
                    context.begin(self.handle, self.width, self.height);
                }
            }
        }
        if !instances.is_empty() { flush(context, &mut instances)?; }
        else { context.discard(); }
        Ok(())
    }
}
