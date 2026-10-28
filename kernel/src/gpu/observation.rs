//! CPU observation of device memory must invalidate before its first read.
//! The transport fence has already completed when the caller reaches this boundary.
//! Host tests: `tests/gpu_observation.rs`.
pub trait Memory {
    fn invalidate(&self, offset: usize, length: usize);
    fn copy(&self, offset: usize, out: &mut [u8]);
}

pub fn read(memory: &impl Memory, offset: usize, out: &mut [u8]) {
    memory.invalidate(offset, out.len());
    memory.copy(offset, out);
}

use crate::gpu_completion::CeCompletion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceVersion {
    pub context: u64,
    pub epoch: u64,
    pub resource: u64,
    pub generation: u64,
}

/// Validated native evidence for M8's public ABI row. No virtual
/// response status is manufactured for a native engine release.
pub struct NativeRecord {
    pub version: ResourceVersion,
    pub writer: CeCompletion,
    pub transfers: [CeCompletion; 2],
    pub bytes: usize,
    pub unwritten: u32,
}

/// Publish only after both complete poisoned transfers describe the same
/// resource version as its completed writer. The backend retains that resource
/// while its serialized bring-up caller issues only read-back transfers. The
/// hold pins lifetime; it does not itself exclude concurrent writers.
pub fn native_record(version: ResourceVersion, writer: CeCompletion, transfers: [CeCompletion; 2],
    first: &[u8], second: &[u8]) -> Result<NativeRecord, &'static str> {
    let requests = [writer.request(), transfers[0].request(), transfers[1].request()];
    if requests.iter().any(|r| r.context != version.context || r.epoch != version.epoch
        || r.resource != version.resource || r.generation != version.generation) {
        return Err("native observation resource version mismatch");
    }
    if requests[0].sequence >= requests[1].sequence || requests[1].sequence >= requests[2].sequence
        || requests[0].caller == requests[1].caller || requests[0].caller == requests[2].caller
        || requests[1].caller == requests[2].caller {
        return Err("native observation completion order mismatch");
    }
    if first.is_empty() || first.len() != second.len() || differing(first, second) != 0 {
        return Err("native observation incomplete or unwritten");
    }
    Ok(NativeRecord { version, writer, transfers, bytes: first.len(), unwritten: 0 })
}

/// Count differing bytes in equal-length observation spans, words first.
pub fn differing(a: &[u8], b: &[u8]) -> u32 {
    assert_eq!(a.len(), b.len());
    let (wa, wb) = (a.chunks_exact(8), b.chunks_exact(8));
    let mut count = wa.remainder().iter().zip(wb.remainder()).filter(|(p, q)| p != q).count() as u32;
    for (x, y) in wa.zip(wb) {
        let d = u64::from_le_bytes(x.try_into().unwrap()) ^ u64::from_le_bytes(y.try_into().unwrap());
        if d != 0 { count += d.to_le_bytes().iter().filter(|&&b| b != 0).count() as u32; }
    }
    count
}

impl NativeRecord {
    /// Map validated native receipts into record v2. The common observation
    /// owner supplies geometry, render serial, pixels, binding and publication;
    /// no scanout flag or present completion is inferred from a CE transfer.
    pub fn write_backend_fields(&self, out: &mut [u8; gpu_abi::record::SIZE], upload: bool) {
        use gpu_abi::{backend, gk208, record as r};
        let put32 = |out: &mut [u8], off: usize, value: u32| out[off..off + 4].copy_from_slice(&value.to_le_bytes());
        let put64 = |out: &mut [u8], off: usize, value: u64| out[off..off + 8].copy_from_slice(&value.to_le_bytes());
        put32(out, r::BACKEND, backend::GK208);
        put32(out, r::BOUNDARY, r::BOUNDARY_NATIVE);
        put64(out, r::CONTEXT, self.version.context);
        put64(out, r::RESOURCE, self.version.resource);
        put64(out, r::GENERATION, self.version.generation);
        put64(out, r::RENDER_FENCE, self.writer.request().caller);
        put64(out, r::READBACK_FENCE, self.transfers[1].request().caller);
        put32(out, r::RENDER_KIND, if upload { gk208::TRANSFER_RELEASE } else { gk208::DRAW_RELEASE });
        for off in [r::RENDER_STATUS, r::READBACK_STATUS, r::COPY_STATUS] { put32(out, off, gk208::SUCCESS); }
        for off in [r::READBACK_KIND, r::COPY_KIND] { put32(out, off, gk208::TRANSFER_RELEASE); }
        put64(out, r::READBACK_SURFACE, 0);
        put64(out, r::READBACK_MOB, 0);
        put32(out, r::UNWRITTEN, self.unwritten);
    }
}
