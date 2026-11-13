"""A destroyed reserved source requests a new scene, not GPU context recovery."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class DisappearingSnapshot(unittest.TestCase):
    def test_destroyed_window_is_stale_then_absent_from_the_next_admission(self):
        source = (ROOT / 'kernel/src/window/scene.rs').read_text()
        start = source.index('    // Check resource metadata before taking the registry;')
        body = source[start:source.index('    drop(identities);', start) + len('    drop(identities);')]
        run_rust(self, r'''
use std::{collections::BTreeMap, sync::{Arc, Mutex, MutexGuard}};
const ENOENT:i32=2;const EAGAIN:i32=11;const ENOMEM:i32=12;
struct Target;
impl Target {fn identity(&self)->Result<(u64,u64,u64),i32>{Ok((3,4,5))}}
struct Buffer {mem:Arc<Target>}
struct Window {front:Option<usize>,buffers:Vec<Buffer>}
struct Registry {windows:BTreeMap<u64,Window>}
struct Lock(Mutex<Registry>);
impl Lock {fn lock(&self)->MutexGuard<'_,Registry>{self.0.lock().unwrap()}}
static REGISTRY:Lock=Lock(Mutex::new(Registry{windows:BTreeMap::new()}));
struct Key {id:u64}
struct Wire {target_generation:u64}
struct Source {key:Key,resource_epoch:u64,wire:Wire}
struct Reservation {sources:Vec<Source>}
fn source()->Source{Source{key:Key{id:7},resource_epoch:3,wire:Wire{target_generation:4}}}
fn admit(reservation:Reservation)->Result<(),i32>{
@BODY@
Ok(())
}
#[test]fn close_between_snapshot_and_submit(){
 // The snapshot reserved window 7; destroy removes it before submit's lookup.
 let reservation=Reservation{sources:vec![source()]};
 REGISTRY.lock().windows.insert(7,Window{front:Some(0),buffers:vec![Buffer{mem:Arc::new(Target)}]});
 REGISTRY.lock().windows.remove(&7);
 assert_eq!(admit(reservation),Err(EAGAIN),"client close was classified as a fatal frame error");
 // compose's existing EAGAIN branch takes another snapshot: no removed source.
 assert_eq!(admit(Reservation{sources:vec![]}),Ok(()));
 // A live source still goes through the original identity validation.
 REGISTRY.lock().windows.insert(7,Window{front:Some(0),buffers:vec![Buffer{mem:Arc::new(Target)}]});
 assert_eq!(admit(Reservation{sources:vec![source()]}),Ok(()));
 let mut stale=source();stale.wire.target_generation=6;
 assert_eq!(admit(Reservation{sources:vec![stale]}),Err(EAGAIN));
 assert_ne!(ENOENT,EAGAIN);
}
'''.replace('@BODY@', body))


if __name__ == '__main__':
    unittest.main()
