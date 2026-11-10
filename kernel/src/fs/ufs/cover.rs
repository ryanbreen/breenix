//! Inode identities for covered directories, independent of pathname spelling.
#[derive(Clone, Copy, Debug)]
pub struct Cover<K> {
    pub mount: u64,
    pub covered: K,
    pub parent: K,
}

pub fn enter<'a, K: Eq + 'a>(covers: impl Iterator<Item = &'a Cover<K>>, key: K) -> Option<u64> {
    covers.filter(|cover| cover.covered == key).map(|cover| cover.mount).next()
}

pub fn leave<'a, K: Copy + 'a>(covers: impl Iterator<Item = &'a Cover<K>>, mount: u64) -> Option<K> {
    covers.filter(|cover| cover.mount == mount).map(|cover| cover.parent).next()
}
