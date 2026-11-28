//! What an x86-64 TLB shootdown request makes each CPU execute, with no
//! kernel dependencies, so `tools/kmem-host-tests` compiles this file as is.
//!
//! `tlb.rs` encodes a `Request` into the word pair its request slots carry,
//! and each CPU, local and remote alike, carries the pair out through
//! `execute` on its own `TlbOps`. The host tests run the same `execute` on a
//! TLB that follows Intel SDM vol. 3A §4.10.4.1.

/// `invlpg` this address.
pub const KIND_PAGE: u8 = 1;
/// Reload CR3, which drops the non-global translations.
pub const KIND_ALL: u8 = 2;
/// Leave this page-table root if it is loaded and this CPU is not returning to it.
pub const KIND_RELEASE_ROOT: u8 = 3;
/// Clear and restore CR4.PGE, which drops global translations too.
pub const KIND_GLOBAL_ALL: u8 = 4;
/// `invlpg` each of the pages in a list. The kind is this base plus the
/// page count, 1 to `MAX_GLOBAL_PAGES`, and the argument is the list's address.
pub const KIND_GLOBAL_PAGES: u8 = 0x20;

/// The most pages a kernel unmap flushes one `invlpg` at a time; a larger
/// batch clears and restores CR4.PGE instead (E1 plan, 6.5).
pub const MAX_GLOBAL_PAGES: usize = 32;

/// A shootdown request, as an initiator names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `invlpg` this address.
    Page(u64),
    /// Reload CR3, which drops the non-global translations.
    All,
    /// Leave this page-table root if it is loaded and this CPU is not
    /// returning to it.
    ReleaseRoot(u64),
    /// `invlpg` each page of the list at `pages`, `count` of them (1 to
    /// `MAX_GLOBAL_PAGES`). `invlpg` removes a page's global translations too.
    GlobalPages { pages: *const u64, count: u8 },
    /// Clear and restore CR4.PGE.
    GlobalAll,
}

/// The TLB operations one CPU can perform on itself.
pub trait TlbOps {
    /// `invlpg`: the page's translations, global or not.
    fn invlpg(&mut self, va: u64);
    /// Reload CR3 with its current value: each non-global translation.
    fn reload_cr3(&mut self);
    /// Clear CR4.PGE and restore it, which invalidates each translation,
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
        Request::GlobalPages { pages, count } => (KIND_GLOBAL_PAGES + count, pages as u64),
        Request::GlobalAll => (KIND_GLOBAL_ALL, 0),
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
        KIND_GLOBAL_ALL => cpu.toggle_global_pages(),
        _ if kind > KIND_GLOBAL_PAGES && usize::from(kind - KIND_GLOBAL_PAGES) <= MAX_GLOBAL_PAGES => {
            let count = usize::from(kind - KIND_GLOBAL_PAGES);
            // SAFETY: `encode` put a list of `count` pages at `arg`, which
            // the caller keeps live.
            let pages = unsafe { core::slice::from_raw_parts(arg as *const u64, count) };
            for &page in pages {
                cpu.invlpg(page);
            }
        }
        _ => {}
    }
}

/// The flush a kernel unmap of `pages` sends to each CPU, the initiator
/// included, before the frames behind them are released. Kernel window
/// pages are global, and a CR3 reload keeps global translations (SDM vol. 3A
/// §4.10.4.1), so the flush is `invlpg` per page for up to
/// `MAX_GLOBAL_PAGES` pages and a CR4.PGE toggle above that.
///
/// The request names `pages`, which the caller keeps live until each CPU
/// has acknowledged it.
pub fn kernel_unmap_request(pages: &[u64]) -> Request {
    match pages.len() {
        count @ 1..=MAX_GLOBAL_PAGES => Request::GlobalPages { pages: pages.as_ptr(), count: count as u8 },
        _ => Request::GlobalAll,
    }
}
