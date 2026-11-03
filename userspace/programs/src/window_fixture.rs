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
    /// Copy the declared section-24 input pattern into the acquired GPU target.
    pub fn paint(self, rect: [i32; 4], seed: u32) -> Result<(), String> {
        use std::cell::RefCell;
        struct Renderer {
            context: bgl::Context,
            input: Option<(u8, bgl::Texture)>,
        }
        thread_local! {
            static RENDER: RefCell<(u64, Option<Renderer>)> = const { RefCell::new((0, None)) };
        }
        let pid = libbreenix::process::getpid().map_err(|e| format!("fixture pid: {e}"))?.raw();
        RENDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.0 != pid { slot.1 = None; slot.0 = pid; }
            if slot.1.is_none() {
                slot.1 = Some(Renderer {
                    context: bgl::Context::new().map_err(|r| format!("fixture context: {r}"))?,
                    input: None,
                });
            }
            let renderer = slot.1.as_mut().expect("fixture renderer created");
            let seed = seed as u8;
            if renderer.input.as_ref().is_none_or(|(old, _)| *old != seed) {
                renderer.input = None;
                let bytes = std::fs::read(format!("/usr/libexec/breenix/share/breengel/window-patterns/{seed:02x}.png"))
                    .map_err(|e| format!("read declared window input {seed}: {e}"))?;
                let image = libimg::Decoded::png(&bytes).map_err(|e| format!("decode declared window input: {e:?}"))?;
                let texture = bgl::Texture::from_decoded(&renderer.context, image)
                    .map_err(|r| format!("upload declared window input: {r}"))?;
                renderer.input = Some((seed, texture));
            }
            let texture = renderer.input.as_ref().expect("declared input loaded").1.handle();
            let context = &mut renderer.context;
            context.discard();
            context.begin(self.handle, self.width, self.height);
            let [x, y, w, h] = rect;
            let (mut y, y1) = (y.max(0), (y as i64 + h as i64).min(self.height as i64) as i32);
            let (x0, x1) = (x.max(0), (x as i64 + w as i64).min(self.width as i64) as i32);
            let mut copies = Vec::new();
            while y < y1 {
                let bottom = y1.min((y / 256 + 1) * 256);
                let mut x = x0;
                while x < x1 {
                    let right = x1.min((x / 256 + 1) * 256);
                    copies.push([x, y, right, bottom, x % 256, y % 256]);
                    x = right;
                }
                y = bottom;
            }
            if copies.is_empty() { context.discard(); return Ok(()); }
            context.copy(texture, &copies);
            context.submit().and_then(|f| f.wait(2_000_000_000))
                .map_err(|r| format!("gpu-window: fixture copy returned {r}"))
        })
    }
}
