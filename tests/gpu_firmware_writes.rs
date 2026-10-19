//! Actual mutation accounting required by gpu-abi.md's firmware handoff counter.
#[path = "../kernel/src/graphics/firmware_writes.rs"]
mod firmware_writes;
use firmware_writes::{count, handoff, note_range, register, BufferMut};

#[test]
fn count_stores_fills_and_row_copies_even_from_access_obtained_before_handoff() {
    let mut framebuffer = [0u8; 32];
    let mut access = BufferMut::new(&mut framebuffer, true);
    access.fill(0..32, 7);
    assert_eq!(count(), 0);
    handoff();
    // Obtaining access, inspecting it and no-op batch rows perform no store.
    assert_eq!(access.len(), 32);
    access.copy_from_slice(0, &[]);
    access.fill(4..4, 9);
    access.copy_within(3..3, 0);
    assert_eq!(count(), 0);
    // Each row copy is a distinct operation, even if it stores existing bytes.
    access.copy_from_slice(0, &[7; 4]);
    access.copy_from_slice(8, &[7; 4]);
    assert_eq!(count(), 2);
    access.fill(12..20, 9);
    access.copy_within(12..20, 20);
    assert_eq!(count(), 4);
    assert_eq!(&access[20..28], &[9; 8]);
    register(access.as_ptr() as usize, access.len());
    let destination = access.as_ptr() as *mut u8;
    let mut shadow = [0; 8];
    let mut access = BufferMut::new(&mut shadow, false);
    access.fill(0..8, 1);
    access.copy_from_slice(0, &[2; 4]);
    access.copy_within(0..4, 4);
    assert_eq!(count(), 4);
    note_range(destination, 0);
    note_range(unsafe { destination.add(32) }, 1);
    note_range(shadow.as_mut_ptr(), shadow.len());
    assert_eq!(count(), 4);
    // The raw-copy caller accounts each nonempty destination range separately.
    note_range(destination, 4);
    note_range(unsafe { destination.add(28) }, 4);
    assert_eq!(count(), 6);
    let mut access = BufferMut::new(&mut framebuffer, true);
    access.fill_pixels(0..10, &[1, 2, 3, 4]);
    assert_eq!(&access[..8], &[1, 2, 3, 4, 1, 2, 3, 4]);
    assert_eq!(count(), 7);
    access.fill_pixels(0..0, &[1, 2, 3, 4]);
    access.fill_pixels(0..3, &[1, 2, 3, 4]);
    access.fill_pixels(0..4, &[]);
    assert_eq!(count(), 7);
    access.render(|frame| frame[..4].fill(5));
    assert_eq!(&access[..4], &[5; 4]);
    assert_eq!(count(), 8);
    let mut shadow = [0; 4];
    BufferMut::new(&mut shadow, false).render(|frame| frame.fill(6));
    assert_eq!(shadow, [6; 4]);
    assert_eq!(count(), 8);
}
