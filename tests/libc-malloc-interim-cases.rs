#[test]
fn interim_warm_malloc_free_issues_no_syscalls() {
    use stdlib::malloc::*;
    unsafe {
        let warm = malloc(80);
        assert!(!warm.is_null());
        free(warm);
        SYSCALLS.store(0, Relaxed);
        for _ in 0..64 {
            let p = malloc(80);
            assert_eq!(p, warm);
            free(p);
        }
        assert_eq!(SYSCALLS.load(Relaxed), 0);
        __breenix_malloc_release_cached();
    }
}

#[test]
fn interim_contended_malloc_waits_and_reuses_a_slot() {
    use stdlib::{alloc_cache, malloc::*};
    let (held, entered) = std::sync::mpsc::sync_channel(0);
    let (notify, event) = std::sync::mpsc::sync_channel(0);
    let holder = std::thread::spawn(move || unsafe {
        let class = alloc_cache::class(2048).unwrap();
        let mut waited = false;
        let (p, tag) = alloc_cache::take(class, |len| {
            let base = mmap(std::ptr::null_mut(), len, 0, 0, -1, 0);
            held.send(()).unwrap();
            waited = event.recv().unwrap();
            assert!(!base.is_null());
            base
        })
        .unwrap();
        (p.addr(), tag, waited)
    });
    entered.recv().unwrap();
    *CONTENDED.lock().unwrap() = Some(notify);
    let p = unsafe { malloc(2048) };
    let (first, first_arena, waited) = holder.join().unwrap();
    assert!(!p.is_null());
    let tag = unsafe { p.cast::<usize>().sub(1).read() };
    assert!(waited, "contender used mmap before a futex wait");
    assert_eq!(
        tag, first_arena,
        "contender allocated outside the holder's arena"
    );
    unsafe {
        free(p);
        alloc_cache::put_run(
            first_arena,
            std::ptr::with_exposed_provenance_mut(first),
            2048,
            |p, n| munmap(p, n) == 0,
        );
        __breenix_malloc_release_cached();
    }
}

#[test]
fn interim_calloc_clears_only_recycled_storage() {
    use stdlib::malloc::*;
    unsafe {
        CLEARS.store(0, Relaxed);
        let small = calloc(32, 4);
        let large = calloc(1, 65_536);
        for (p, len) in [(small, 128), (large, 65_536)] {
            assert!(!p.is_null());
            assert!(std::slice::from_raw_parts(p, len).iter().all(|&b| b == 0));
        }
        assert_eq!(CLEARS.load(Relaxed), 0);
        small.write_bytes(0xa5, 128);
        free(small);
        let reused = calloc(128, 1);
        assert_eq!(reused, small);
        assert!(std::slice::from_raw_parts(reused, 128)
            .iter()
            .all(|&b| b == 0));
        assert_eq!(CLEARS.load(Relaxed), 128);
        free(reused);
        free(large);
        __breenix_malloc_release_cached();
    }
}

#[test]
fn interim_realloc_claims_and_releases_adjacent_slots() {
    use stdlib::malloc::*;
    unsafe {
        let p = malloc(64);
        let blocked = malloc(64);
        let free_slot = malloc(64);
        assert!(!free_slot.is_null());
        p.write_bytes(0xa5, 64);
        free(blocked);
        free(free_slot);
        let grown = realloc(p, 180);
        assert_eq!(grown, p);
        assert!(std::slice::from_raw_parts(grown, 64)
            .iter()
            .all(|&b| b == 0xa5));
        assert!(malloc_usable_size(grown) >= 180);
        grown.write_bytes(0x5a, 180);
        let next = malloc(64);
        assert!(next.addr() >= grown.addr() + 180);
        assert_eq!(realloc(grown, 64), grown);
        let reused = malloc(64);
        assert_eq!(reused, blocked);
        free(reused);
        free(next);
        free(grown);
        __breenix_malloc_release_cached();
    }
}
