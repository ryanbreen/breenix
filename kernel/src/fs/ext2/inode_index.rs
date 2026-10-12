//! An ordered inode index whose insertion reports allocation failure.
use alloc::vec::Vec;
use core::ops::Bound;
pub(super) struct InodeIndex<T> { entries: Vec<(u32,T)> }
impl<T> InodeIndex<T> {
    pub const fn new()->Self{Self{entries:Vec::new()}}
    pub fn len(&self)->usize{self.entries.len()}
    pub fn get(&self,key:&u32)->Option<&T>{self.entries.binary_search_by_key(key,|(key,_)|*key).ok().map(|i|&self.entries[i].1)}
    pub fn insert(&mut self,key:u32,value:T)->Result<(),&'static str>{
        match self.entries.binary_search_by_key(&key,|(key,_)|*key){
            Ok(i)=>self.entries[i].1=value,
            Err(i)=>{self.entries.try_reserve(1).map_err(|_|"No memory for ext2 inode index")?;self.entries.insert(i,(key,value));}
        }Ok(())
    }
    pub fn remove(&mut self,key:&u32)->Option<T>{self.entries.binary_search_by_key(key,|(key,_)|*key).ok().map(|i|self.entries.remove(i).1)}
    pub fn values(&self)->impl ExactSizeIterator<Item=&T>{self.entries.iter().map(|(_,value)|value)}
    pub fn range(&self,bounds:(Bound<u32>,Bound<u32>))->impl Iterator<Item=(&u32,&T)>{
        let start=match bounds.0{Bound::Unbounded=>0,Bound::Included(k)=>self.entries.partition_point(|(key,_)|*key<k),Bound::Excluded(k)=>self.entries.partition_point(|(key,_)|*key<=k)};
        let end=match bounds.1{Bound::Unbounded=>self.entries.len(),Bound::Included(k)=>self.entries.partition_point(|(key,_)|*key<=k),Bound::Excluded(k)=>self.entries.partition_point(|(key,_)|*key<k)};
        self.entries[start.min(end)..end].iter().map(|(key,value)|(key,value))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    #[test]
    fn ordered_lookup_removal_and_cursor_ranges_match_a_tree(){
        let mut index=InodeIndex::new();let mut tree=BTreeMap::new();
        for n in [12,3,20,0,u32::MAX,15,3,8,9,11]{index.insert(n,n).unwrap();tree.insert(n,n);}
        assert_eq!(index.len(),tree.len());
        for n in [0,1,3,8,12,20,21,u32::MAX]{assert_eq!(index.get(&n),tree.get(&n));}
        for n in [0,3,12,20,u32::MAX]{
            for bounds in [(Bound::Excluded(n),Bound::Unbounded),(Bound::Unbounded,Bound::Included(n))]{
                assert_eq!(index.range(bounds).collect::<Vec<_>>(),tree.range(bounds).collect::<Vec<_>>());
            }
        }
        for n in [3,20,30,0]{assert_eq!(index.remove(&n),tree.remove(&n));}
        assert_eq!(index.values().copied().collect::<Vec<_>>(),tree.values().copied().collect::<Vec<_>>());
    }
}
