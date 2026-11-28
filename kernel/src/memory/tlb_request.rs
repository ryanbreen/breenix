//! What an x86-64 TLB shootdown request makes each CPU execute, with no
//! kernel dependencies, so `tools/kmem-host-tests` compiles this file as is.
//!
//! `tlb.rs` encodes a `Request` into the word pair its request slots carry,
//! and each CPU, local and remote alike, carries the pair out through
//! `execute` on its own `TlbOps`. The host tests run the same `execute` on a
//! TLB that follows Intel SDM vol. 3A §4.10.4.1.

/// `invlpg` this address.
pub const KIND_PAGE: u8 = 1;
/// Reload CR3: every non-global translation.
pub const KIND_ALL: u8 = 2;
/// Leave this page-table root if it is loaded and this CPU is not returning to it.
pub const KIND_RELEASE_ROOT: u8 = 3;

/// A shootdown request, as an initiator names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `invlpg` this address.
    Page(u64),
    /// Reload CR3: every non-global translation.
    All,
    /// Leave this page-table root if it is loaded and this CPU is not
    /// returning to it.
    ReleaseRoot(u64),
}

/// The TLB operations one CPU can perform on itself.
pub trait TlbOps {
    /// `invlpg`: the page's translations, global or not.
    fn invlpg(&mut self, va: u64);
    /// Reload CR3 with its current value: each non-global translation.
    fn reload_cr3(&mut self);
    /// Clear CR4.PGE and restore it, which invalidates every translation,
    /// global ones included. With PGE already clear no translation is
    /// global, and a CR3 reload does the same.
    fn toggle_global_pages(&mut self);
    /// Leave page-table root `root` (`tlb.rs`'s `release_root`).
    fn release_root(&mut self, root: u64);
}

/// The `(kind, arg)` pair a request slot carries.
pub fn encode(request: Request) -> (u8, u64) {
    match request {
        Request::Page(addr) => (KIND_PAGE, addr),
        Request::All => (KIND_ALL, 0),
        Request::ReleaseRoot(root) => (KIND_RELEASE_ROOT, root),
    }
}

/// Carry out the request in a slot on the executing CPU.
///
/// # Safety
/// `(kind, arg)` came from `encode`, and memory the request names stays
/// valid until the call returns.
pub unsafe fn execute(kind: u8, arg: u64, cpu: &mut impl TlbOps) {
    match kind {
        KIND_PAGE => cpu.invlpg(arg),
        KIND_ALL => cpu.reload_cr3(),
        KIND_RELEASE_ROOT => cpu.release_root(arg),
        _ => {}
    }
}

/// The flush a kernel unmap of `pages` sends to each CPU before the frames
/// behind them are released. `unmap_kernel_page` leaves this to its caller,
/// which flushes once on every CPU with `tlb::flush_all`.
pub fn kernel_unmap_request(_pages: &[u64]) -> Request {
    Request::All
}
