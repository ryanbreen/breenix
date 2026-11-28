//! E3-1: the two-list LRU scan (plan sections 8.3 and 12.1).
use std::cell::Cell;
use std::collections::BTreeSet;

use pagecache_host_tests::lru::{mark_referenced, Links, Lru, PageLinks, ISOLATED};
use pagecache_host_tests::Lcg;

/// Link words for pages 0..n, as FrameMeta's `use` words hold them.
struct Pages(Vec<Cell<Links>>);

impl PageLinks for Pages {
    fn links(&self, page: u64) -> Links {
        self.0[page as usize].get()
    }

    fn set_links(&mut self, page: u64, links: Links) {
        self.0[page as usize].set(links);
    }

    fn compare_exchange_bits(&self, page: u64, current: u8, new: u8) -> Result<(), u8> {
        let word = &self.0[page as usize];
        let mut links = word.get();
        if links.bits != current {
            return Err(links.bits);
        }
        links.bits = new;
        word.set(links);
        Ok(())
    }
}

const CAPACITY: usize = 256;
const WORKING_SET: u64 = 64;
const STREAM_BASE: u64 = 1000;
const STREAM_LEN: u64 = 10 * CAPACITY as u64;

/// A cache of `CAPACITY` pages under the LRU. A miss reclaims through the
/// scan when full, evicting everything it isolates, then inserts.
struct Cache {
    lru: Lru,
    pages: Pages,
    resident: BTreeSet<u64>,
    evicted: Vec<u64>,
}

impl Cache {
    fn new() -> Cache {
        Cache {
            lru: Lru::new(),
            pages: Pages(vec![Cell::new(Links::UNLINKED); (STREAM_BASE + STREAM_LEN) as usize]),
            resident: BTreeSet::new(),
            evicted: Vec::new(),
        }
    }

    fn access(&mut self, page: u64) {
        if self.resident.contains(&page) {
            mark_referenced(&self.pages, page);
            return;
        }
        while self.resident.len() >= CAPACITY {
            self.lru.age_active(&mut self.pages, 32);
            let mut batch = [0u64; 32];
            let n = self.lru.scan_inactive(&mut self.pages, 64, &mut batch);
            for &victim in &batch[..n] {
                assert_ne!(self.pages.links(victim).bits & ISOLATED, 0);
                self.lru.evicted(&mut self.pages, victim).unwrap();
                self.resident.remove(&victim);
                self.evicted.push(victim);
            }
        }
        self.lru.insert(&mut self.pages, page).unwrap();
        self.resident.insert(page);
        assert_eq!(self.lru.inactive_len() + self.lru.active_len(), self.resident.len());
    }
}

fn shuffled(rng: &mut Lcg, mut pages: Vec<u64>) -> Vec<u64> {
    for i in (1..pages.len()).rev() {
        pages.swap(i, rng.below(i as u64 + 1) as usize);
    }
    pages
}

#[test]
fn a_working_set_read_twice_survives_a_stream_ten_times_the_cache_seed_2c0ffee() {
    let mut rng = Lcg(0x2c0_ffee);
    let mut cache = Cache::new();
    for _ in 0..2 {
        for page in shuffled(&mut rng, (0..WORKING_SET).collect()) {
            cache.access(page);
        }
    }
    // The stream, read once, with an occasional re-read of the working set.
    for page in STREAM_BASE..STREAM_BASE + STREAM_LEN {
        cache.access(page);
        if rng.below(8) == 0 {
            cache.access(rng.below(WORKING_SET));
        }
    }
    let lost: Vec<_> = (0..WORKING_SET).filter(|p| !cache.resident.contains(p)).collect();
    assert!(lost.is_empty(), "working-set pages evicted: {lost:?}");
    // Only stream pages were evicted, oldest first.
    assert!(cache.evicted.len() as u64 >= STREAM_LEN - CAPACITY as u64);
    let mut expected = STREAM_BASE;
    for &page in &cache.evicted {
        assert_eq!(page, expected, "eviction order");
        expected += 1;
    }
}

#[test]
fn a_page_read_once_is_evicted_before_a_page_read_twice() {
    let mut cache = Cache::new();
    // Fill the cache with pages read once, except page 0, which is read twice
    // and is the oldest.
    cache.access(0);
    for page in 1..CAPACITY as u64 {
        cache.access(page);
    }
    cache.access(0);
    cache.access(STREAM_BASE);
    assert!(cache.resident.contains(&0));
    assert_eq!(cache.evicted.first(), Some(&1));

    // Without the second read, the oldest page goes first.
    let mut cold = Cache::new();
    for page in 0..=CAPACITY as u64 {
        cold.access(page);
    }
    assert_eq!(cold.evicted.first(), Some(&0));
}
