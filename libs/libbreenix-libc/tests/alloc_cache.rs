// Run independently of the freestanding libc (whose exported malloc replaces
// the host allocator): rustc --edition=2021 --test tests/alloc_cache.rs -o /tmp/alloc-cache-tests
// /tmp/alloc-cache-tests --test-threads=1
pub fn getpid() -> i32 {
    std::process::id() as i32
}
#[path = "../src/alloc_cache.rs"]
mod alloc_cache;

#[test]
fn size_classes_preserve_capacity_and_header_tags() {
    for size in 1..=2048 {
        let class = alloc_cache::class(size).unwrap();
        assert!(alloc_cache::capacity(class) >= size);
        assert_eq!(
            alloc_cache::marked_class(alloc_cache::marker(class)),
            Some(class)
        );
    }
    assert_eq!(alloc_cache::class(0), None);
    assert_eq!(alloc_cache::class(2049), None);
    assert_eq!(alloc_cache::marked_class(0), None);
    assert_eq!(alloc_cache::marked_class(0x40000000), None);
}
