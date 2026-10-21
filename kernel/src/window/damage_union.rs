//! Exact non-overlapping scene damage, clipped with wide coordinate arithmetic.
use alloc::vec::Vec;
use breengel_abi::{errno::ENOMEM, Rect};

/// Intersect two half-open rectangles.
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = (a.x as i64).max(b.x as i64);
    let y0 = (a.y as i64).max(b.y as i64);
    let x1 = (a.x as i64 + a.w.max(0) as i64).min(b.x as i64 + b.w.max(0) as i64);
    let y1 = (a.y as i64 + a.h.max(0) as i64).min(b.y as i64 + b.h.max(0) as i64);
    (x1 > x0 && y1 > y0)
        .then(|| Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

pub fn normalized(input: &[Rect], screen: Rect) -> Result<Vec<Rect>, i32> {
    let mut out = Vec::new();
    for rect in input.iter().filter_map(|&r| intersect(r, screen)) {
        let mut pieces = alloc::vec![rect];
        for &prior in &out {
            let mut next = Vec::new();
            for p in pieces {
                if let Some(i) = intersect(p, prior) {
                    next.try_reserve(4).map_err(|_| ENOMEM)?;
                    next.extend(
                        [
                            Rect::new(p.x, p.y, p.w, i.y - p.y),
                            Rect::new(p.x, i.y + i.h, p.w, p.y + p.h - i.y - i.h),
                            Rect::new(p.x, i.y, i.x - p.x, i.h),
                            Rect::new(i.x + i.w, i.y, p.x + p.w - i.x - i.w, i.h),
                        ]
                        .into_iter()
                        .filter(|r| !r.is_empty()),
                    );
                } else {
                    next.try_reserve(1).map_err(|_| ENOMEM)?;
                    next.push(p);
                }
            }
            pieces = next;
        }
        out.try_reserve(pieces.len()).map_err(|_| ENOMEM)?;
        out.extend(pieces);
    }
    Ok(out)
}
