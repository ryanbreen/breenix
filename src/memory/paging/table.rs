use memory::paging::entry::*;
use memory::paging::ENTRY_COUNT;

pub const P4: *mut Table = 0xffffffff_fffff000 as *mut _;

pub trait TableLevel {}

pub enum Level4 {}
enum Level3 {}
enum Level2 {}
enum Level1 {}

impl TableLevel for Level4 {}
impl TableLevel for Level3 {}
impl TableLevel for Level2 {}
impl TableLevel for Level1 {}

trait HierachicalLevel: TableLevel {}

impl HierachicalLevel for Level4 {}
impl HierachicalLevel for Level3 {}
impl HierachicalLevel for Level2 {}

use core::marker::PhantomData;

pub struct Table<L: TableLevel> {
    entries: [Entry; ENTRY_COUNT],
    level: PhantomData<L>,
}

use core::ops::{Index, IndexMut};

impl Index<usize> for Table {
  type Output = Entry;

  fn index(&self, index: usize) -> &Entry {
    &self.entries[index]
  }

  pub fn zero(&mut self) {
    for entry in self.entries.iter_mut() {
      entry.set_unused();
    }
  }
  fn next_table_address(&self, index: usize) -> Option<usize> {
    let entry_flags = self[index].flags();
    if entry_flags.contains(PRESENT) && !entry_flags.contains(HUGE_PAGE) {
      let table_address = self as *const _ as usize;
      Some((table_address << 9) | (index << 12))
    } else {
      None
    }
  }
  pub fn next_table(&self, index: usize) -> Option<&Table> {
    self.next_table_address(index)
        .map(|address| unsafe { &*(address as *const _) })
  }

  pub fn next_table_mut(&mut self, index: usize) -> Option<&mut Table> {
    self.next_table_address(index)
        .map(|address| unsafe { &mut *(address as *mut _) })
  }
}

impl IndexMut<usize> for Table {
  fn index_mut(&mut self, index: usize) -> &mut Entry {
    &mut self.entries[index]
  }
}
