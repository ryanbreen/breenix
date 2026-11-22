#!/usr/bin/env python3
"""Run only allocation, exit and random tests with host-safe exported names."""
from pathlib import Path
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='libc-stdlib-host-') as tmp:
    directory = Path(tmp)
    sources = ROOT / 'libs/libbreenix-libc/src/stdlib'
    for name in ('malloc', 'exit', 'rand', 'alloc_cache'):
        text = (sources / (name + '.rs')).read_text().replace('#[no_mangle]', '').replace('_Exit', 'exit_now')
        (directory / (name + '.rs')).write_text(text)
    string_source = (ROOT / 'libs/libbreenix-libc/src/string/mod.rs').read_text()
    start = string_source.index('pub unsafe extern "C" fn strerror_l')
    end = string_source.index('#[no_mangle]', start)
    strerror_l = string_source[start:end]
    source = r'''
#![deny(warnings)]
include!("__MALLOC_HOST__");
mod pthread { pub(crate) use crate::set_errno; }
mod string {
    #[allow(dead_code)] // Other error/signal APIs are outside this focused harness.
    #[path="__ERRTEXT__"] mod errtext;
    __STRERROR_L__
    pub use crate::allocation_string::{memcpy, memset};
}
mod stdio { pub unsafe fn flush_all() {} }
fn exit_group(_: i32) -> ! { panic!("unexpected process exit") }
unsafe fn sigprocmask(_: i32, _: *const u64, _: *mut u64) {}
fn raise(_: i32) {}
fn signal(_: i32, _: usize) {}
#[allow(dead_code)] // Published C APIs outside this focused test subset.
mod stdlib {
    pub(crate) use crate::{errno, set_errno};
    pub const EINVAL: i32 = 22;
    pub const ENOMEM: i32 = 12;
    #[path="__TMP__/alloc_cache.rs"] pub mod alloc_cache;
    #[path="__TMP__/malloc.rs"] pub mod malloc;
    #[path="__TMP__/exit.rs"] pub mod exit;
    #[path="__TMP__/rand.rs"] pub mod rand;
}
#[test]
fn thousands_of_live_small_blocks_share_backing_mappings() {
    use stdlib::malloc::{malloc, free, __breenix_malloc_release_cached};
    const BLOCKS: usize = 4096;
    const MAX_MAPPINGS: usize = 128;
    unsafe {
        let blocks: Vec<_> = (0..BLOCKS).map(|i| {
            let size = [16, 31, 47, 63][i % 4];
            let p = malloc(size);
            assert!(!p.is_null());
            assert_eq!(p as usize % 16, 0);
            p.cast::<usize>().write(i);
            p.add(size - 1).write(0xa5);
            p
        }).collect();
        for (i, &p) in blocks.iter().enumerate() {
            assert_eq!(p.cast::<usize>().read(), i);
            assert_eq!(p.add([16, 31, 47, 63][i % 4] - 1).read(), 0xa5);
        }
        let mappings = MAPPINGS.lock().unwrap().len();
        for p in blocks { free(p); }
        __breenix_malloc_release_cached();
        assert!(MAPPINGS.lock().unwrap().is_empty());
        assert!(mappings <= MAX_MAPPINGS,
                "{BLOCKS} live allocations used {mappings} mappings (limit {MAX_MAPPINGS})");
        eprintln!("{BLOCKS} live allocations used {mappings} mappings (limit {MAX_MAPPINGS})");
    }
}
#[test]
fn cache_release_discards_free_pages_in_a_live_arena() {
    use stdlib::malloc::{malloc, free, __breenix_malloc_release_cached};
    unsafe {
        let blocks: Vec<_> = (0..60).map(|_| {
            let ptr = malloc(4096);
            assert!(!ptr.is_null());
            ptr.write_bytes(0xa5, 4096);
            ptr
        }).collect();
        for &ptr in &blocks[1..59] { free(ptr); }
        assert_eq!(__breenix_malloc_release_cached(), 0);
        let discarded: usize = DISCARDED.lock().unwrap().drain(..).map(|(_, len)| len).sum();
        for ptr in [blocks[0], blocks[59]] {
            assert!(std::slice::from_raw_parts(ptr, 4096).iter().all(|&b| b == 0xa5));
            free(ptr);
        }
        assert_eq!(__breenix_malloc_release_cached(), 1);
        assert!(MAPPINGS.lock().unwrap().is_empty());
        assert!(discarded >= 56 * 4096, "released only {discarded} bytes of free pages");
    }
}
#[test]
fn mid_size_realloc_and_calloc_preserve_data_and_release_mappings() {
    use stdlib::malloc::*;
    unsafe {
        let ptr = malloc(3000);
        let peer = malloc(3500);
        assert!(!ptr.is_null() && !peer.is_null());
        assert_eq!((ptr as usize % 16, peer as usize % 16), (0, 0));
        assert_eq!(MAPPINGS.lock().unwrap().len(), 1);
        ptr.write_bytes(0xa5, 3000);
        assert_eq!(realloc(ptr, 3800), ptr);
        assert!(realloc(ptr, usize::MAX).is_null());
        assert_eq!(errno(), 12);
        let large = realloc(ptr, 65536);
        assert!(!large.is_null());
        assert!(std::slice::from_raw_parts(large, 3000).iter().all(|&b| b == 0xa5));
        assert_eq!(MAPPINGS.lock().unwrap().len(), 2);
        assert_eq!(__breenix_malloc_release_cached(), 0);
        let zeroed = calloc(100, 30);
        assert_eq!(zeroed, ptr);
        assert!(std::slice::from_raw_parts(zeroed, 3000).iter().all(|&b| b == 0));
        assert!(calloc(usize::MAX, 2).is_null());
        assert_eq!(errno(), 12);
        set_errno(77);
        free(large);
        assert_eq!(errno(), 77);
        assert_eq!(MAPPINGS.lock().unwrap().len(), 1);
        free(zeroed);
        free(peer);
        assert_eq!(__breenix_malloc_release_cached(), 1);
        assert!(MAPPINGS.lock().unwrap().is_empty());
    }
}
#[test]
fn alignment_overflow_preserves_output_and_errno() {
    unsafe {
        let mut p = 123usize as *mut u8;
        set_errno(77);
        assert_eq!(stdlib::malloc::posix_memalign(&mut p, 1usize << 63, 1usize << 63), 12);
        assert_eq!(p as usize, 123);
        assert_eq!(errno(), 77);
        assert!(MAPPINGS.lock().unwrap().is_empty());
    }
}
#[test]
fn overaligned_free_releases_all_pages() {
    unsafe {
        for alignment in [32, 4096, 65536] {
            let p = stdlib::malloc::aligned_alloc(alignment, 10000);
            assert!(!p.is_null());
            assert_eq!(p as usize % alignment, 0);
            p.write(0x5a);
            p.add(9999).write(0xa5);
            stdlib::malloc::free(p);
            assert!(MAPPINGS.lock().unwrap().is_empty());
        }
    }
}
#[test]
fn random_repeats_but_has_its_own_feedback_state() {
    use stdlib::rand::*;
    srandom(17);
    let a: Vec<_> = (0..100).map(|_| random()).collect();
    srandom(17);
    assert_eq!(a, (0..100).map(|_| random()).collect::<Vec<_>>());
    assert!(a.iter().all(|&x| (0..=i32::MAX as i64).contains(&x)));
    srand(17);
    assert_ne!(a, (0..100).map(|_| rand() as i64).collect::<Vec<_>>());
}
#[test]
fn seed48_can_restore_its_returned_seed_buffer() {
    unsafe {
        stdlib::rand::srand48(0);
        let mut seed = [1u16,2,3];
        let old = stdlib::rand::seed48(seed.as_mut_ptr());
        assert_eq!(*old.cast::<[u16;3]>(), [0x330e,0,0]);
        stdlib::rand::srand48(17);
        assert_eq!(*old.cast::<[u16;3]>(), [0x330e,0,0]);
        let mut expected = [0x330eu16,0,0];
        stdlib::rand::seed48(old);
        assert_eq!(stdlib::rand::drand48(), stdlib::rand::erand48(expected.as_mut_ptr()));
    }
}
#[test]
fn concurrent_48bit_initialization_and_generation() {
    let threads: Vec<_> = (0..8).map(|i| std::thread::spawn(move || unsafe {
        let mut x = [i, 1, 2];
        for j in 0..1000 {
            stdlib::rand::srand48(j);
            let v = stdlib::rand::erand48(x.as_mut_ptr());
            assert!((0.0..1.0).contains(&v));
            assert!(stdlib::rand::nrand48(x.as_mut_ptr()) >= 0);
            stdlib::rand::jrand48(x.as_mut_ptr());
            let mut params = [1,2,3,4,5,6,7];
            stdlib::rand::lcong48(params.as_mut_ptr());
            stdlib::rand::seed48(x.as_mut_ptr());
            stdlib::rand::drand48();
        }
    })).collect();
    for t in threads { t.join().unwrap(); }
}
'''.replace('__TMP__', str(directory)).replace('__MALLOC_HOST__', str(ROOT / 'tests/fixtures/libc_malloc_host.rs')).replace('__ERRTEXT__', str(ROOT / 'libs/libbreenix-libc/src/string/errtext.rs')).replace('__STRERROR_L__', strerror_l)
    source += r'''
#[test]
fn strerror_l_has_immutable_terminated_thread_safe_text() {
    let workers: Vec<_> = (0..8).map(|i| std::thread::spawn(move || {
        for _ in 0..1000 {
            unsafe {
                let p = string::strerror_l(-i-1, std::ptr::null_mut());
                assert_eq!(std::ffi::CStr::from_ptr(p.cast()).to_bytes(), b"Unknown error");
                assert_eq!(errno(), 22);
                let known = string::strerror_l(22, std::ptr::null_mut());
                assert_eq!(std::ffi::CStr::from_ptr(known.cast()).to_bytes(), b"An argument is invalid");
            }
        }
    })).collect();
    for w in workers { w.join().unwrap(); }
}
'''
    src = directory / 'tests.rs' 
    src.write_text(source)
    binary = directory / 'tests'
    subprocess.run(['rustc', '--edition=2021', '--test', str(src), '-o', str(binary)], check=True)
    subprocess.run([str(binary), '--test-threads=1', '--show-output'], check=True)
