//! The boot-time range set. Each backing page is itself reserved before use.

use arm64_boot_contract::phys_memory::{PhysRange, PAGE_SIZE};
use core::ptr::{self, NonNull};

#[derive(Clone, Copy)]
struct Entry {
    range: PhysRange,
    pending: bool,
}

const ENTRIES_PER_PAGE: usize = (PAGE_SIZE as usize - 16) / size_of::<Entry>();

#[repr(C)]
struct Page {
    next: *mut Page,
    phys: u64,
    entries: [Entry; ENTRIES_PER_PAGE],
}

const _: () = assert!(size_of::<Page>() <= PAGE_SIZE as usize);

/// # Safety
/// A returned pointer grants exclusive use of one mapped, writable RAM page
/// for the lifetime of the pool. Callers have already excluded live memory.
pub unsafe trait RangeStorage {
    fn storage_page(&self, phys: u64) -> Option<NonNull<u8>>;
}

/// Sorted, disjoint, page-aligned usable ranges. Mutation is boot-CPU-only;
/// consumers share the immutable set after publication.
pub struct RangeSet {
    first: *mut Page,
    last: *mut Page,
    pages: usize,
    len: usize,
}

// The pages are owned by the set and never returned to the allocator.
unsafe impl Send for RangeSet {}
unsafe impl Sync for RangeSet {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    NoMappedStorage,
    InvalidRequest,
    NoFit,
    /// Usable RAM above the 52-bit physical address limit.
    AboveAddressLimit,
}

impl RangeError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoMappedStorage => "no mapped writable RAM for boot range storage",
            Self::InvalidRequest => "invalid boot carve size or alignment",
            Self::NoFit => "no boot range fits requested carve",
            Self::AboveAddressLimit => "usable RAM above the 52-bit physical address limit",
        }
    }
}

impl RangeSet {
    pub const fn new() -> Self {
        Self {
            first: ptr::null_mut(),
            last: ptr::null_mut(),
            pages: 0,
            len: 0,
        }
    }

    fn entry(&self, index: usize) -> Entry {
        // Each caller reads only initialized entries.
        unsafe { self.slot(index).read() }
    }

    fn slot(&self, index: usize) -> *mut Entry {
        debug_assert!(index < self.pages * ENTRIES_PER_PAGE);
        let mut page = self.first;
        for _ in 0..index / ENTRIES_PER_PAGE {
            page = unsafe { (*page).next };
        }
        unsafe {
            ptr::addr_of_mut!((*page).entries)
                .cast::<Entry>()
                .add(index % ENTRIES_PER_PAGE)
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = PhysRange> + '_ {
        let mut page = self.first;
        let mut offset = 0;
        (0..self.len).map(move |_| {
            let entry = unsafe {
                ptr::addr_of!((*page).entries)
                    .cast::<Entry>()
                    .add(offset)
                    .read()
            };
            offset += 1;
            if offset == ENTRIES_PER_PAGE {
                page = unsafe { (*page).next };
                offset = 0;
            }
            entry.range
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn total_bytes(&self) -> u64 {
        self.iter().map(|range| range.len()).sum()
    }

    fn append_page(&mut self, phys: u64, address: NonNull<u8>) {
        let page = address.as_ptr().cast::<Page>();
        unsafe {
            ptr::addr_of_mut!((*page).next).write(ptr::null_mut());
            ptr::addr_of_mut!((*page).phys).write(phys);
            if self.last.is_null() {
                self.first = page;
            } else {
                (*self.last).next = page;
            }
        }
        self.last = page;
        self.pages += 1;
    }

    fn replace(&mut self, start: usize, count: usize, entries: &[Entry]) {
        let tail = start + count..self.len;
        if entries.len() > count {
            let distance = entries.len() - count;
            for index in tail.rev() {
                unsafe { self.slot(index + distance).write(self.entry(index)) };
            }
        } else if entries.len() < count {
            let distance = count - entries.len();
            for index in tail {
                unsafe { self.slot(index - distance).write(self.entry(index)) };
            }
        }
        for (index, entry) in entries.iter().copied().enumerate() {
            unsafe { self.slot(start + index).write(entry) };
        }
        self.len = self.len - count + entries.len();
    }

    fn insert_raw(&mut self, mut entry: Entry) {
        let mut first = 0;
        while first < self.len {
            let before = self.entry(first);
            if before.range.end > entry.range.start
                || (before.range.end == entry.range.start && before.pending == entry.pending)
            {
                break;
            }
            first += 1;
        }
        let mut end = first;
        while end < self.len {
            let after = self.entry(end);
            if after.range.start > entry.range.end
                || (after.range.start == entry.range.end && after.pending != entry.pending)
            {
                break;
            }
            debug_assert_eq!(
                after.pending, entry.pending,
                "staged release overlaps usable RAM"
            );
            entry.range.start = entry.range.start.min(after.range.start);
            entry.range.end = entry.range.end.max(after.range.end);
            end += 1;
        }
        self.replace(first, end - first, &[entry]);
    }

    fn remove(&mut self, index: usize, cut: PhysRange) {
        let entry = self.entry(index);
        let before = Entry {
            range: PhysRange::new(entry.range.start, cut.start),
            ..entry
        };
        let after = Entry {
            range: PhysRange::new(cut.end, entry.range.end),
            ..entry
        };
        match (before.range.is_empty(), after.range.is_empty()) {
            (true, true) => self.replace(index, 1, &[]),
            (false, true) => self.replace(index, 1, &[before]),
            (true, false) => self.replace(index, 1, &[after]),
            (false, false) => self.replace(index, 1, &[before, after]),
        }
    }

    fn grow(&mut self, storage: &impl RangeStorage, window: PhysRange) -> Result<(), RangeError> {
        for index in (0..self.len).rev() {
            let entry = self.entry(index);
            if entry.pending {
                continue;
            }
            if let Some((phys, address)) = writable_page(intersect(entry.range, window), storage) {
                self.append_page(phys, address);
                self.remove(index, PhysRange::new(phys, phys + PAGE_SIZE));
                return Ok(());
            }
        }
        Err(RangeError::NoMappedStorage)
    }

    /// Remove the highest aligned run ending at or below the exclusive limit.
    pub fn carve(&mut self, bytes: u64, align: u64, below: u64) -> Result<PhysRange, RangeError> {
        if bytes == 0 || bytes % PAGE_SIZE != 0 || align < PAGE_SIZE || !align.is_power_of_two() {
            return Err(RangeError::InvalidRequest);
        }
        for index in (0..self.len).rev() {
            let entry = self.entry(index);
            let Some(base) = entry.range.end.min(below).checked_sub(bytes) else {
                continue;
            };
            let start = base & !(align - 1);
            if !entry.pending && start >= entry.range.start {
                let cut = PhysRange::new(start, start + bytes);
                // Builders keep spare entries for the boot pool carves.
                if self.len == self.pages * ENTRIES_PER_PAGE
                    && start > entry.range.start
                    && cut.end < entry.range.end
                {
                    return Err(RangeError::NoMappedStorage);
                }
                self.remove(index, cut);
                return Ok(cut);
            }
        }
        Err(RangeError::NoFit)
    }

    #[cfg(any(target_arch = "aarch64", test))]
    pub(super) fn carve_mapped_page(
        &mut self,
        below: u64,
        storage: &impl RangeStorage,
    ) -> Result<u64, RangeError> {
        for index in (0..self.len).rev() {
            let range = intersect(self.entry(index).range, PhysRange::new(0, below));
            if let Some((phys, _)) = writable_page(range, storage) {
                return self.carve(PAGE_SIZE, PAGE_SIZE, phys + PAGE_SIZE).map(|cut| cut.start);
            }
        }
        Err(RangeError::NoFit)
    }

    #[cfg(any(target_arch = "aarch64", test))]
    pub(super) fn restore_carved_page(&mut self, phys: u64) {
        self.insert_raw(Entry { range: PhysRange::new(phys, phys + PAGE_SIZE), pending: false });
    }

    #[cfg(any(target_arch = "aarch64", test))]
    pub(super) fn retain_below(&mut self, below: u64) {
        while self.len != 0 {
            let index = self.len - 1;
            let mut entry = self.entry(index);
            if entry.range.start >= below {
                self.len -= 1;
            } else {
                entry.range.end = entry.range.end.min(below);
                unsafe { self.slot(index).write(entry) };
                break;
            }
        }
    }

    fn insert(
        &mut self,
        mut range: PhysRange,
        pending: bool,
        storage: &impl RangeStorage,
        window: PhysRange,
    ) -> Result<(), RangeError> {
        while !range.is_empty() {
            // Keep room for an insertion, a growth-page split and the two
            // fixed boot-time pool carves.
            if self.len + 4 >= self.pages * ENTRIES_PER_PAGE {
                self.grow(storage, window)?;
            }
            // The first page is reserved before the second pass, and a growth
            // page may come from an interval that a later input overlaps.
            let mut exclude = range.end;
            let mut page = self.first;
            while !page.is_null() {
                let phys = unsafe { (*page).phys };
                if phys >= range.start {
                    exclude = exclude.min(phys);
                }
                page = unsafe { (*page).next };
            }
            if range.start < exclude {
                self.insert_raw(Entry {
                    range: PhysRange::new(range.start, exclude),
                    pending,
                });
            }
            if exclude == range.end {
                break;
            }
            range.start = exclude + PAGE_SIZE;
        }
        Ok(())
    }

    fn release_pending(&mut self) {
        let mut out: usize = 0;
        for index in 0..self.len {
            let mut entry = self.entry(index);
            entry.pending = false;
            if let Some(previous) = out.checked_sub(1) {
                let mut before = self.entry(previous);
                if before.range.end == entry.range.start {
                    before.range.end = entry.range.end;
                    unsafe { self.slot(previous).write(before) };
                    continue;
                }
            }
            unsafe { self.slot(out).write(entry) };
            out += 1;
        }
        self.len = out;
    }
}

pub fn intersect(a: PhysRange, b: PhysRange) -> PhysRange {
    PhysRange::new(a.start.max(b.start), a.end.min(b.end))
}

pub fn align_in(range: PhysRange) -> PhysRange {
    let start = range
        .start
        .checked_next_multiple_of(PAGE_SIZE)
        .unwrap_or(u64::MAX);
    PhysRange::new(start, range.end & !(PAGE_SIZE - 1))
}

fn writable_page(range: PhysRange, storage: &impl RangeStorage) -> Option<(u64, NonNull<u8>)> {
    let mut end = range.end;
    while end > range.start {
        end -= PAGE_SIZE;
        if let Some(page) = storage.storage_page(end) {
            return Some((end, page));
        }
    }
    None
}

type Emit<'a> = dyn FnMut(PhysRange, bool) -> Result<(), RangeError> + 'a;

pub fn build<E: From<RangeError>>(
    storage: &impl RangeStorage,
    window: PhysRange,
    mut stream: impl FnMut(&mut Emit<'_>) -> Result<(), E>,
) -> Result<RangeSet, E> {
    let mut largest = PhysRange::default();
    let mut first_page = None;
    stream(&mut |range, pending| {
        let range = align_in(intersect(range, window));
        if !pending && range.len() > largest.len() {
            if let Some(page) = writable_page(range, storage) {
                largest = range;
                first_page = Some(page);
            }
        }
        Ok(())
    })?;
    let (phys, pointer) = first_page.ok_or(RangeError::NoMappedStorage)?;
    let mut set = RangeSet::new();
    set.append_page(phys, pointer);
    // Seed the largest safe run so storage can grow even when the first
    // descriptors in the second pass are all tiny or staged for release.
    set.insert(largest, false, storage, window)?;
    stream(&mut |range, pending| set.insert(align_in(range), pending, storage, window))?;
    set.release_pending();
    Ok(set)
}

#[cfg(any(target_arch = "x86_64", test))]
pub fn from_x86(
    storage: &impl RangeStorage,
    usable: impl Iterator<Item = PhysRange> + Clone,
) -> Result<RangeSet, RangeError> {
    let window = PhysRange::new(0x10_0000, u64::MAX);
    build(storage, window, |emit| {
        for range in usable.clone() {
            emit(intersect(range, window), false)?;
        }
        Ok(())
    })
}

#[cfg(any(target_arch = "aarch64", test))]
#[path = "boot_ranges_arm64.rs"]
mod arm64;
#[cfg(any(target_arch = "aarch64", test))]
pub use arm64::*;
