pub fn getpid() -> i32 {
    std::process::id() as i32
}
#[path = "../libs/libbreenix-libc/src/stdlib/alloc_cache.rs"]
mod alloc_cache;

#[test]
fn size_classes_preserve_capacity_and_alignment() {
    for size in 1..=32768 {
        let class = alloc_cache::class(size).unwrap();
        let capacity = alloc_cache::capacity(class);
        assert!(capacity >= size);
        assert_eq!(capacity % 16, 0);
        assert_eq!(alloc_cache::class(capacity), Some(class));
    }
    assert_eq!(alloc_cache::class(0), None);
    assert_eq!(alloc_cache::class(32769), None);
    assert!(!alloc_cache::is_arena_tag(0));
    assert!(!alloc_cache::is_arena_tag(0x40000000));
}
