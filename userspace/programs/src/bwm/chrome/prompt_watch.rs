//! Keep semantic publication enabled while a prompt receipt is outstanding.
use libbreenix::{memory, window::{self, abi::{self, op}}};

struct Mapping(abi::MapTree);
impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = memory::munmap(self.0.addr as *mut u8, self.0.len as usize);
    }
}

#[derive(Default)]
pub(crate) struct Watchers {
    mappings: Vec<Mapping>,
}
impl Watchers {
    pub(crate) fn attach(&mut self, id: u64) -> Result<(), String> {
        if self.mappings.iter().any(|map| map.0.id == id) { return Ok(()); }
        let mut map = abi::MapTree { id, ..Default::default() };
        window::call(op::MAP_TREE, &mut map).map_err(|error| error.to_string())?;
        self.mappings.push(Mapping(map));
        Ok(())
    }

    pub(crate) fn retain(&mut self, mut pending: impl FnMut(u64) -> bool) {
        self.mappings.retain(|map| pending(map.0.id));
    }
}
