extern crate alloc;
#[path = "../kernel/src/window/damage_union.rs"]
mod damage_union;
use breengel_abi::Rect;

#[test]
fn normalized_damage_covers_the_exact_clipped_union_once() {
    let screen = Rect::new(0, 0, 19, 13);
    let input = [
        Rect::new(-4, 2, 9, 8), Rect::new(3, -2, 8, 9),
        Rect::new(7, 5, 20, 6), Rect::new(3, 4, 8, 6),
        Rect::new(0, 0, 0, 3), Rect::new(i32::MAX-2, 1, 12, 4),
        Rect::new(i32::MIN, 3, i32::MAX, 3),
    ];
    let output = damage_union::normalized(&input, screen).unwrap();
    let contains = |r: &Rect, x: i64, y: i64| r.w > 0 && r.h > 0
        && x >= r.x as i64 && x < r.x as i64 + r.w as i64
        && y >= r.y as i64 && y < r.y as i64 + r.h as i64;
    for y in -2..16 { for x in -2..22 {
        let want = contains(&screen,x,y) && input.iter().any(|r| contains(r,x,y));
        let count = output.iter().filter(|r| contains(r,x,y)).count();
        assert_eq!(count, usize::from(want), "damage coverage at ({x},{y})");
    }}
}
