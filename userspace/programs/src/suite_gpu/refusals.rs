use libbreenix::gpu;
use gpu::abi::{self, record, stream};
use stream::encode;

/// The handle-taking ops, each given `h`, with what each returned (EINVAL is required).
pub(super) fn refusals_for(h: u64, own_ctx: u64) -> Vec<(&'static str, i64)> {
    let mut out = Vec::new();
    out.push(("CONTEXT_DESTROY", gpu::context_destroy(h)));
    out.push(("RESOURCE_DESTROY", gpu::resource_destroy(h)));
    out.push(("SUBMIT on it as a context", gpu::submit(h, &[stream::cmd::CLEAR, 3, 0]).0));
    out.push(("WAIT on it as a context", gpu::wait(h, 1 << 32 | 1, 0).0));
    // COUNTERS of context 0 is the calling process's (gpu-abi.md, COUNTERS).
    if h != 0 {
        out.push(("COUNTERS of it as a context", gpu::counters(h).0));
    }
    let mut px = vec![0u8; 64];
    let mut rec = [0u8; record::SIZE];
    out.push(("READBACK of it", gpu::readback(h, [0, 0, 4, 4], &mut px, 16, &mut rec)));
    out.push(("UPLOAD into it", gpu::upload(own_ctx, h, [0, 0, 1, 1], &[0; 4], 4, 0, abi::purpose::IMAGE).0));
    out.push(("UPLOAD in it as a context", gpu::upload(h, h, [0, 0, 1, 1], &[0; 4], 4, 0, abi::purpose::IMAGE).0));
    let mut s = Vec::new();
    encode::begin(&mut s, h);
    let (r, a) = gpu::submit(own_ctx, &s);
    // A refusal naming anything but the handle at command 0 is not this refusal.
    let named = a.error == stream::error::BAD_HANDLE && a.error_index == 0;
    out.push(("BEGIN of it, code 4 at command 0", if named { r } else { 0 }));
    out
}

