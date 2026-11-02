//! Cumulative window-observer provenance; timing drains do not consume it.
pub struct Scanout {
    words: [u64; 14],
    binding: [u64; 2],
    latched: u64,
}
impl Scanout {
    pub const fn new() -> Self {
        let mut words = [0; 14]; words[0] = 1;
        Self { words, binding: [0; 2], latched: 0 }
    }
    fn add(&mut self, index: usize, value: u64) {
        match self.words[index].checked_add(value) {
            Some(sum) => self.words[index] = sum,
            None => self.words[0] = 0, // Refuse the snapshot instead of wrapping.
        }
    }
    pub fn binding(&mut self, requested: u64, actual: u64) {
        self.binding = [requested, actual];
    }
    pub fn latched(&mut self, resource: u64) { self.latched = resource; }
    /// Called after writing record v2, with its RESOURCE field read from the bytes.
    pub fn published(&mut self, recorded: u64) {
        self.add(1, 1);
        self.words[8] = recorded;
        let latched = self.latched;
        self.words[7] = latched;
        if latched != 0 && latched == recorded { self.add(2, 1); }
        else { self.add(9, 1); }
        let [requested, actual] = self.binding;
        if requested != 0 && actual != requested && latched == actual {
            self.add(10, 1);
            self.words[11..14].copy_from_slice(&[requested, latched, recorded]);
        }
    }
    pub fn result(&mut self, confirmed: bool) { if !confirmed { self.add(3, 1); } }
    pub fn wait(&mut self, pending: bool, elapsed: u64) {
        if pending {
            self.add(4, 1); self.add(5, elapsed);
            self.words[6] = self.words[6].max(elapsed);
        }
    }
    pub fn snapshot(&self) -> [u64; 14] { self.words }
}
