//! E3-1: a lock-free hit that races a reclaim scan (plan sections 12.1 and
//! 12.3). The hit runs without the zone lock, so a scan on another CPU can
//! isolate the page between the hit reading the page's LRU word and writing
//! it back. The harness runs that scan at exactly that point.
use std::cell::RefCell;

use pagecache_host_tests::lru::{
    mark_referenced, Links, Lru, PageLinks, ACTIVE, ISOLATED, NIL, ON_LRU, REFERENCED,
};

struct Table(Vec<Links>);

impl PageLinks for Table {
    fn links(&self, page: u64) -> Links {
        self.0[page as usize]
    }

    fn set_links(&mut self, page: u64, links: Links) {
        self.0[page as usize] = links;
    }
}

/// The link words of one zone, and a scan that runs on another CPU the moment
/// the hit on `race_page` writes.
struct Racing {
    table: RefCell<Table>,
    lru: RefCell<Lru>,
    race_page: Option<u64>,
    isolated: RefCell<Vec<u64>>,
}

impl Racing {
    /// The other CPU: one inactive scan of one page, under the zone lock.
    fn scan(&self) {
        let mut batch = [NIL; 1];
        let n = self
            .lru
            .borrow_mut()
            .scan_inactive(&mut *self.table.borrow_mut(), 1, &mut batch);
        self.isolated.borrow_mut().extend_from_slice(&batch[..n]);
    }
}

impl PageLinks for Racing {
    fn links(&self, page: u64) -> Links {
        self.table.borrow().links(page)
    }

    fn set_links(&mut self, page: u64, links: Links) {
        if self.race_page == Some(page) {
            self.race_page = None;
            self.scan();
        }
        self.table.get_mut().set_links(page, links);
    }
}

#[test]
fn a_hit_racing_a_scan_keeps_the_isolation_and_its_reference() {
    let mut racing = Racing {
        table: RefCell::new(Table(vec![Links::UNLINKED; 3])),
        lru: RefCell::new(Lru::new()),
        race_page: None,
        isolated: RefCell::new(Vec::new()),
    };
    for page in 0..3 {
        racing
            .lru
            .get_mut()
            .insert(racing.table.get_mut(), page)
            .unwrap();
    }
    // Page 0 is the inactive tail. The hit on it reads its word, then the
    // scan isolates it, then the hit's write lands.
    racing.race_page = Some(0);
    mark_referenced(&mut racing, 0);
    assert_eq!(*racing.isolated.borrow(), vec![0], "the scan isolated page 0");

    let table = racing.table.get_mut();
    let lru = racing.lru.get_mut();
    assert_eq!(
        table.links(0),
        Links {
            prev: NIL,
            next: NIL,
            bits: ISOLATED | REFERENCED,
        },
        "the hit kept the scan's isolation and added its reference"
    );
    assert_eq!(table.links(1).next, NIL, "page 1 is the new inactive tail");
    assert_eq!(table.links(1).bits, ON_LRU);
    assert_eq!((lru.inactive_len(), lru.active_len()), (2, 0));

    // Reclaim's completion accepts the isolated page, and the reference
    // survives the putback: the next scan promotes it and isolates the rest.
    lru.putback(table, 0, false).unwrap();
    let mut batch = [NIL; 3];
    assert_eq!(lru.scan_inactive(table, 3, &mut batch), 2);
    assert_eq!(&batch[..2], &[1, 2]);
    assert_eq!((lru.inactive_len(), lru.active_len()), (0, 1));
    assert_eq!(table.links(0).bits, ON_LRU | ACTIVE);
}
