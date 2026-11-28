//! The page cache's LRU approximation (E3 plan, sections 12.1 and 12.3).
//!
//! Two lists of file pages, `inactive` and `active`. New pages enter
//! `inactive` at its head unreferenced, which is the scan-resistant first
//! queue of 2Q (Johnson and Shasha 1994) and the inactive queue of the BSD
//! pageout design (McKusick et al. 2014, ch. 6). A hit only sets
//! `REFERENCED`, so hits take no list lock. The inactive scan, from the tail,
//! promotes referenced pages to `active` and isolates the rest as reclaim
//! candidates. While `inactive` is below half of `active`, the tail of
//! `active` moves to `inactive`, and a referenced active page is rotated once
//! instead (second chance; Corbato 1968).
//!
//! The lists are threaded through per-page link words that the caller stores
//! ([`PageLinks`]; FrameMeta's `use` words in the kernel), so no list
//! operation allocates. A hit changes only the bits word, by
//! compare-and-exchange, so it cannot undo a list change made under the
//! lock. Pages are named by an abstract number (the pfn).
//!
//! The module has no kernel dependency. The kernel supplies the zone lock.

/// The end of a list.
pub const NIL: u64 = u64::MAX;

/// Referenced since the scan last passed it.
pub const REFERENCED: u8 = 1 << 0;
/// On the active list.
pub const ACTIVE: u8 = 1 << 1;
/// On a list.
pub const ON_LRU: u8 = 1 << 2;
/// Taken off the lists by a scan.
pub const ISOLATED: u8 = 1 << 3;

/// One page's list links and LRU bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Links {
    pub prev: u64,
    pub next: u64,
    pub bits: u8,
}

impl Links {
    pub const UNLINKED: Links = Links {
        prev: NIL,
        next: NIL,
        bits: 0,
    };
}

/// Per-page link storage. The bits are one word of their own, apart from
/// `prev` and `next`.
pub trait PageLinks {
    fn links(&self, page: u64) -> Links;
    /// Store a page's links and bits. Only list operations call this, under
    /// the zone lock; the bits word is written with one atomic store.
    fn set_links(&mut self, page: u64, links: Links);
    /// Replace the page's bits word with `new` if it holds `current`, as one
    /// atomic compare-and-exchange of that word alone; on failure return the
    /// bits it holds. A hit uses only this and `links`, so it does not write
    /// `prev`, `next` or a membership bit.
    fn compare_exchange_bits(&self, page: u64, current: u8, new: u8) -> Result<(), u8>;
}

/// Why an LRU operation was refused; the lists are unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LruError {
    AlreadyListed,
    NotListed,
    NotIsolated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct List {
    head: u64,
    tail: u64,
    len: usize,
}

impl List {
    const EMPTY: List = List {
        head: NIL,
        tail: NIL,
        len: 0,
    };

    fn push_head<P: PageLinks>(&mut self, pages: &mut P, page: u64, bits: u8) {
        pages.set_links(
            page,
            Links {
                prev: NIL,
                next: self.head,
                bits,
            },
        );
        if self.head == NIL {
            self.tail = page;
        } else {
            let mut old = pages.links(self.head);
            old.prev = page;
            pages.set_links(self.head, old);
        }
        self.head = page;
        self.len += 1;
    }

    fn unlink<P: PageLinks>(&mut self, pages: &mut P, page: u64) -> Links {
        let links = pages.links(page);
        if links.prev == NIL {
            self.head = links.next;
        } else {
            let mut prev = pages.links(links.prev);
            prev.next = links.next;
            pages.set_links(links.prev, prev);
        }
        if links.next == NIL {
            self.tail = links.prev;
        } else {
            let mut next = pages.links(links.next);
            next.prev = links.prev;
            pages.set_links(links.next, next);
        }
        self.len -= 1;
        links
    }
}

/// The two lists of one zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lru {
    inactive: List,
    active: List,
}

impl Lru {
    pub const fn new() -> Lru {
        Lru {
            inactive: List::EMPTY,
            active: List::EMPTY,
        }
    }

    pub const fn inactive_len(&self) -> usize {
        self.inactive.len
    }

    pub const fn active_len(&self) -> usize {
        self.active.len
    }

    /// A new page enters `inactive` at its head, unreferenced.
    pub fn insert<P: PageLinks>(&mut self, pages: &mut P, page: u64) -> Result<(), LruError> {
        if pages.links(page).bits & (ON_LRU | ISOLATED) != 0 {
            return Err(LruError::AlreadyListed);
        }
        self.inactive.push_head(pages, page, ON_LRU);
        Ok(())
    }

    /// Take a listed page off its list (truncation, unlink).
    pub fn remove<P: PageLinks>(&mut self, pages: &mut P, page: u64) -> Result<(), LruError> {
        let bits = pages.links(page).bits;
        if bits & ON_LRU == 0 {
            return Err(LruError::NotListed);
        }
        if bits & ACTIVE != 0 {
            self.active.unlink(pages, page);
        } else {
            self.inactive.unlink(pages, page);
        }
        pages.set_links(page, Links::UNLINKED);
        Ok(())
    }

    /// Scan `inactive` from its tail, looking at no more than `budget` pages.
    /// Referenced pages move to `active` with the bit cleared; the others are
    /// isolated into `batch`, up to its length. Returns the count isolated.
    pub fn scan_inactive<P: PageLinks>(&mut self, pages: &mut P, budget: usize, batch: &mut [u64]) -> usize {
        let mut isolated = 0;
        let mut looked = 0;
        while looked < budget && isolated < batch.len() && self.inactive.tail != NIL {
            looked += 1;
            let page = self.inactive.tail;
            let links = self.inactive.unlink(pages, page);
            if links.bits & REFERENCED != 0 {
                self.active.push_head(pages, page, ON_LRU | ACTIVE);
            } else {
                pages.set_links(
                    page,
                    Links {
                        prev: NIL,
                        next: NIL,
                        bits: ISOLATED,
                    },
                );
                batch[isolated] = page;
                isolated += 1;
            }
        }
        isolated
    }

    /// Return an isolated page that was not evicted: to `active` when `active`
    /// is set (a page a locked binding covers, so the scan does not meet it
    /// again at once), otherwise to the head of `inactive`.
    pub fn putback<P: PageLinks>(&mut self, pages: &mut P, page: u64, active: bool) -> Result<(), LruError> {
        let bits = pages.links(page).bits;
        if bits & ISOLATED == 0 {
            return Err(LruError::NotIsolated);
        }
        let kept = bits & REFERENCED;
        if active {
            self.active.push_head(pages, page, ON_LRU | ACTIVE | kept);
        } else {
            self.inactive.push_head(pages, page, ON_LRU | kept);
        }
        Ok(())
    }

    /// An isolated page was evicted: it leaves the LRU for good.
    pub fn evicted<P: PageLinks>(&mut self, pages: &mut P, page: u64) -> Result<(), LruError> {
        if pages.links(page).bits & ISOLATED == 0 {
            return Err(LruError::NotIsolated);
        }
        pages.set_links(page, Links::UNLINKED);
        Ok(())
    }

    /// Age `active` while `inactive` is below half of it, looking at no more
    /// than `budget` pages. Returns the pages moved to `inactive`.
    pub fn age_active<P: PageLinks>(&mut self, pages: &mut P, budget: usize) -> usize {
        let mut moved = 0;
        let mut looked = 0;
        while looked < budget && self.inactive.len < self.active.len / 2 && self.active.tail != NIL {
            looked += 1;
            let page = self.active.tail;
            let links = self.active.unlink(pages, page);
            if links.bits & REFERENCED != 0 {
                self.active.push_head(pages, page, ON_LRU | ACTIVE);
            } else {
                self.inactive.push_head(pages, page, ON_LRU);
                moved += 1;
            }
        }
        moved
    }
}

impl Default for Lru {
    fn default() -> Self {
        Lru::new()
    }
}

/// Record a hit: set `REFERENCED` on a listed or isolated page. No list
/// changes, so the kernel does this without the zone lock.
///
/// Only the bits word changes, by compare-and-exchange, so a scan that moves
/// or isolates the page meanwhile keeps its links and membership bits, and
/// the hit retries against the bits the scan left. A list operation's store
/// of a word it read before the hit can drop the hit's `REFERENCED`; that
/// loses one reference hint and no list change.
pub fn mark_referenced<P: PageLinks>(pages: &P, page: u64) {
    let mut bits = pages.links(page).bits;
    while bits & (ON_LRU | ISOLATED) != 0 && bits & REFERENCED == 0 {
        match pages.compare_exchange_bits(page, bits, bits | REFERENCED) {
            Ok(()) => return,
            Err(seen) => bits = seen,
        }
    }
}
