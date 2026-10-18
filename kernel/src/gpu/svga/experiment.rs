//! Black-box experiments V1 and V2 of the GPU core provenance record (scratch only, never
//! merged): constant-buffer surface definitions, and the view, update and copy forms the
//! SVGA3D DX backend uses. Each experiment is one command buffer run to its terminal
//! status; a refused buffer's status and error offset are printed, and the queue restarted.
use super::*;

fn try_cb(dev: &mut Device, name: &str, dx: Option<u32>, words: &[u32]) {
    let r = dev.exec_dx(dx, words);
    crate::serial_println!("[svga-exp] {} -> {:?}", name, r);
}

impl Device {
    fn exec_dx(&mut self, dx: Option<u32>, words: &[u32]) -> Result<u32, (u32, u32)> {
        let (header, payload, id) = self.buffer(dx, words).map_err(|_| (99, 0))?;
        self.issue(&header, 0);
        let start = crate::signal::monotonic_nanos();
        loop {
            header.observe();
            let status = header.u32_at(0);
            if header.u64_at(8) != id { return Err((98, 0)); }
            if status == wire::status::COMPLETED { return Ok(1); }
            if status != wire::status::PENDING {
                let at = header.u32_at(4);
                let _ = self.start_queue();
                return Err((status, at));
            }
            if crate::signal::monotonic_nanos().saturating_sub(start) >= 2_000_000_000 {
                core::mem::forget(header);
                core::mem::forget(payload);
                return Err((97, 0));
            }
            core::hint::spin_loop();
        }
    }
}

/// `SVGA3D_SURFACE_BIND_CONSTANT_BUFFER` (svga3d_types.h:360).
const CONSTANT: u32 = 1 << 22;

pub fn run(dev: &mut Device) {
    let sid = |n: u32| 30000 + n;
    let mut c = Commands::new();
    c.surface(sid(1), CONSTANT, wire::format::BUFFER, 256, 1);
    try_cb(dev, "E1 cb 256", None, c.words());
    let mut c = Commands::new();
    c.surface(sid(2), CONSTANT, wire::format::BUFFER, 4096, 1);
    try_cb(dev, "E2 cb 4096", None, c.words());
    let mut c = Commands::new();
    c.command(1134, &[sid(3), CONSTANT, wire::format::BUFFER, 1, 0, 0, 4096, 1, 1, 1]);
    try_cb(dev, "E3 cb v2", None, c.words());
    let mut c = Commands::new();
    c.surface(sid(4), CONSTANT | (1 << 2), wire::format::BUFFER, 4096, 1);
    try_cb(dev, "E4 cb dynamic", None, c.words());
    let mut c = Commands::new();
    c.surface(sid(5), CONSTANT | (1 << 1), wire::format::BUFFER, 4096, 1);
    try_cb(dev, "E5 cb static", None, c.words());
    let mut c = Commands::new();
    c.surface(sid(6), wire::bind::VERTEX, wire::format::BUFFER, 4096, 1);
    try_cb(dev, "E6 vb 4096", None, c.words());
    let mut c = Commands::new();
    c.surface(sid(7), CONSTANT, 2, 4096, 1);
    try_cb(dev, "E7 cb fmt2", None, c.words());
    let Some(cid) = with_ids(|i| i.cids.take()).flatten() else { return };
    let mut mobs = [0u32; 8];
    for m in mobs.iter_mut() {
        *m = with_ids(|i| i.mobs.take()).flatten().unwrap_or(0);
    }
    let Ok(ctx) = objects::Context::new(cid, mobs, VIEWS, encode::SHADER_IDS) else { return };
    let mut c = Commands::new();
    ctx.define(&mut c);
    try_cb(dev, "X context", None, c.words());
    core::mem::forget(ctx);
    let mut c = Commands::new();
    c.surface(sid(8), wire::bind::SAMPLE, wire::format::BGRA, 1, 1);
    try_cb(dev, "E8 unbacked tex define", None, c.words());
    let mut c = Commands::new();
    c.dx_define_srv(0, sid(8), wire::format::BGRA);
    try_cb(dev, "E9 srv on unbacked", Some(cid), c.words());
    let mut c = Commands::new();
    c.dx_set_shader_resource(0, 2, wire::INVALID);
    try_cb(dev, "E10 srv unbind invalid", Some(cid), c.words());
    let mut c = Commands::new();
    c.dx_set_shader_resource(0, 2, 0);
    try_cb(dev, "E11 srv bind 0", Some(cid), c.words());
    let pages = Arc::new(Pages::new(1).unwrap());
    let tree = PageTree::new(pages).unwrap();
    let mob = with_ids(|i| i.mobs.take()).flatten().unwrap_or(0);
    let mut c = Commands::new();
    c.mob(mob, &tree, 4096);
    c.bind_surface(sid(6), mob);
    try_cb(dev, "E12 bind vb mob", None, c.words());
    core::mem::forget(tree);
    let mut c = Commands::new();
    c.update_surface(sid(6));
    try_cb(dev, "E13 update in dx", Some(cid), c.words());
    let mut c = Commands::new();
    c.surface(sid(9), wire::bind::SAMPLE | wire::bind::RENDER, wire::format::BGRA, 4, 4);
    c.surface(sid(10), wire::bind::SAMPLE | wire::bind::RENDER, wire::format::BGRA, 4, 4);
    try_cb(dev, "E14 two rt", None, c.words());
    let mut c = Commands::new();
    c.copy(sid(9), sid(10), [0, 0, 1, 1], [0, 0]);
    try_cb(dev, "E15 surface copy in dx", Some(cid), c.words());
    let mut c = Commands::new();
    c.dx_copy_region(sid(10), 0, 0, sid(9), 0, 0, 1, 1);
    try_cb(dev, "E16 pred copy region", Some(cid), c.words());
    // DX_SET_SINGLE_CONSTANT_BUFFER (svga3d_cmd.h:171): slot 0, PS, surface, offset, size.
    let mut c = Commands::new();
    c.command(1148, &[0, 2, sid(2), 0, 256]);
    try_cb(dev, "E17 set cb (E2)", Some(cid), c.words());
    let mut c = Commands::new();
    c.command(1148, &[0, 2, sid(6), 0, 256]);
    try_cb(dev, "E18 set cb on vb", Some(cid), c.words());
    let mut c = Commands::new();
    c.surface(sid(11), CONSTANT, wire::format::BUFFER, 4096, 1);
    try_cb(dev, "E19 cb in dx ctx define", Some(cid), c.words());
}
