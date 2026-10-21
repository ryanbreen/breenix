//! Suite-only observation of successful syscall 416 uploads, before failure artifacts.
//! Declarations retain static source bytes; content comparison precedes hash recording.
use super::abi;
use crate::crypto::sha256::sha256;
use std::{collections::BTreeMap, sync::Mutex, vec::Vec};

struct Table {
    rows: Vec<&'static [u8]>,
}
#[derive(Clone, Copy, Default)]
pub struct Mark {
    image: u64,
    fixture: u64,
    refused: u64,
}
#[derive(Default)]
struct Audit {
    active: bool,
    /// Declarations refused since `start` because a row lies outside the program image.
    nonstatic: u64,
    tables: Vec<Table>,
    resources: BTreeMap<u64, abi::ResourceCreate>,
    mark: Mark,
    hashes: Vec<([u8; 32], bool)>,
}
static AUDIT: Mutex<Audit> = Mutex::new(Audit {
    active: false,
    nonstatic: 0,
    tables: Vec::new(),
    resources: BTreeMap::new(),
    mark: Mark {
        image: 0,
        fixture: 0,
        refused: 0,
    },
    hashes: Vec::new(),
});
fn locked() -> std::sync::MutexGuard<'static, Audit> {
    AUDIT.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn start() {
    let mut a = locked();
    let resources = core::mem::take(&mut a.resources);
    *a = Audit {
        active: true,
        resources,
        ..Audit::default()
    };
}
pub fn stop() {
    locked().active = false;
}
pub fn declare(bytes: &'static [u8]) {
    declare_rows(std::vec![bytes]);
}
/// A static table may be stored as separate static rows (the bitmap-font dependency).
/// The rows must lie in the executable's read-only image: a leaked heap allocation or a
/// writable static is refused, whatever its lifetime says.
pub fn declare_rows(rows: Vec<&'static [u8]>) {
    locked().declare(rows, crate::image::read_only);
}
pub fn mark() -> Mark {
    locked().mark
}
pub fn refusal(before: Mark, image: u64, fixture: u64) -> Option<&'static str> {
    locked().refusal(before, image, fixture)
}

pub fn hashes() -> std::string::String {
    use std::fmt::Write;
    let a = locked();
    let mut text = std::string::String::new();
    for (hash, legal) in &a.hashes {
        if !text.is_empty() {
            text.push(',');
        }
        text.push_str(if *legal { "table:" } else { "refused:" });
        for b in hash {
            let _ = write!(text, "{b:02x}");
        }
    }
    if text.is_empty() {
        text.push('-');
    }
    text
}
impl Audit {
    fn declare(&mut self, rows: Vec<&'static [u8]>, read_only: impl Fn(&[u8]) -> bool) {
        if rows.iter().all(|r| read_only(r)) {
            self.tables.push(Table { rows });
        } else {
            self.nonstatic += 1;
        }
    }

    fn refusal(&self, before: Mark, image: u64, fixture: u64) -> Option<&'static str> {
        if image != 0 {
            Some("cpu-image")
        } else if self.nonstatic != 0 {
            // Sticky for the case: declarations precede the marks callers take.
            Some("non-static-table")
        } else if self.mark.refused != before.refused {
            Some("passing-oracle")
        } else if self.mark.image - before.image != image
            || self.mark.fixture - before.fixture != fixture
        {
            Some("unobserved-upload")
        } else {
            None
        }
    }

    fn pixels(&mut self, purpose: u32, bytes: &[u8]) {
        let table = if purpose == abi::purpose::FIXTURE {
            self.tables.iter().position(|t| {
                t.rows.iter().map(|r| r.len()).sum::<usize>() == bytes.len()
                    && t.rows.iter().flat_map(|r| r.iter()).eq(bytes.iter())
            })
        } else {
            None
        };
        // A declaration licenses one upload: the upload consumes it, so a later identical
        // upload (a row-at-a-time fill matching a declared row) needs a declaration of its own.
        match table {
            Some(i) => {
                self.tables.remove(i);
            }
            None => self.mark.refused += 1,
        }
        self.hashes.push((sha256(bytes), table.is_some()));
    }
}
/// Called only after a successful GPU syscall. Successful uploads have readable clipped
/// source rows. Resource extents determine exactly those rows, including for raw uploads.
///
/// # Safety
/// `arg` has the validity required by `gpu::call` and remains live after that call.
pub(super) unsafe fn observe(op: u32, arg: *mut u8, len: usize) {
    observe_in(&mut locked(), op, arg, len);
}
unsafe fn observe_in(a: &mut Audit, op: u32, arg: *mut u8, len: usize) {
    match op {
        abi::op::RESOURCE_CREATE if len == core::mem::size_of::<abi::ResourceCreate>() => {
            let r = *(arg as *const abi::ResourceCreate);
            a.resources.insert(r.handle, r);
        }
        abi::op::RESOURCE_DESTROY if len == core::mem::size_of::<abi::Handle>() => {
            a.resources.remove(&(*(arg as *const abi::Handle)).handle);
        }
        abi::op::UPLOAD if a.active && len == core::mem::size_of::<abi::Upload>() => {
            let u = *(arg as *const abi::Upload);
            let Some(r) = a.resources.get(&u.resource).copied() else {
                a.mark.refused += 1;
                return;
            };
            let (w, h) = if r.kind == abi::kind::BUFFER {
                (r.size, 1)
            } else {
                (r.width as u64, r.height as u64)
            };
            let cw = (u.width as u64).min(w.saturating_sub(u.x as u64));
            let ch = (u.height as u64).min(h.saturating_sub(u.y as u64));
            let row = cw * abi::upload_source::bytes(u.source) as u64;
            let n = row * ch;
            if u.purpose == abi::purpose::FIXTURE {
                a.mark.fixture += n;
            } else {
                a.mark.image += n;
            }
            // Buffer bytes (crossing tables included) are content-checked like pixels: CPU
            // results encoded as geometry are as much a CPU render as an uploaded image.
            let mut bytes = Vec::with_capacity(n as usize);
            for y in 0..ch {
                bytes.extend_from_slice(core::slice::from_raw_parts(
                    (u.pixels + y * u.stride as u64) as *const u8,
                    row as usize,
                ));
            }
            // A clipped UPLOAD is a partial table, even if its visible bytes match another table.
            if cw != u.width as u64 || ch != u.height as u64 {
                a.mark.refused += 1;
            }
            a.pixels(u.purpose, &bytes);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::Plain;
    static FONT: &[u8] = &[14, 17, 17, 31, 17, 17, 17, 4, 12, 4, 4, 4, 4, 14];
    fn audit() -> Audit {
        Audit {
            active: true,
            tables: std::vec![Table {
                rows: std::vec![FONT]
            }],
            ..Audit::default()
        }
    }
    fn upload(a: &mut Audit, bytes: &[u8], purpose: u32, extent: u32) {
        let mut r = abi::ResourceCreate {
            handle: 1,
            kind: abi::kind::TEXTURE,
            width: extent,
            height: 1,
            format: abi::format::R8,
            ..abi::ResourceCreate::ZERO
        };
        let mut u = abi::Upload {
            resource: 1,
            width: bytes.len() as u32,
            height: 1,
            pixels: bytes.as_ptr() as u64,
            pixels_len: bytes.len() as u64,
            stride: bytes.len() as u32,
            source: abi::upload_source::R8,
            purpose,
            ..abi::Upload::ZERO
        };
        // Simulate the successful syscall boundary, including resource metadata and payload capture.
        unsafe {
            observe_in(
                a,
                abi::op::RESOURCE_CREATE,
                &mut r as *mut _ as *mut u8,
                core::mem::size_of_val(&r),
            );
            observe_in(
                a,
                abi::op::UPLOAD,
                &mut u as *mut _ as *mut u8,
                core::mem::size_of_val(&u),
            );
        }
    }
    #[test]
    fn cpu_rendered_image_tagged_fixture_fails() {
        let mut a = audit();
        let before = a.mark;
        let rendered = [17, 34, 51, 255].repeat(32 * 24);
        upload(
            &mut a,
            &rendered,
            abi::purpose::FIXTURE,
            rendered.len() as u32,
        );
        assert_eq!(
            a.refusal(before, 0, rendered.len() as u64),
            Some("passing-oracle")
        );
        assert_eq!(a.hashes, std::vec![(sha256(&rendered), false)]);
        // Equal length, a partial table and Image purpose cannot masquerade as a table.
        for (bytes, purpose) in [
            (&[0; 14][..], abi::purpose::FIXTURE),
            (&FONT[..7], abi::purpose::FIXTURE),
            (FONT, abi::purpose::IMAGE),
        ] {
            let before = a.mark;
            upload(&mut a, bytes, purpose, bytes.len() as u32);
            assert_eq!(
                a.refusal(
                    before,
                    if purpose == abi::purpose::IMAGE {
                        bytes.len() as u64
                    } else {
                        0
                    },
                    if purpose == abi::purpose::FIXTURE {
                        bytes.len() as u64
                    } else {
                        0
                    }
                ),
                Some(if purpose == abi::purpose::IMAGE {
                    "cpu-image"
                } else {
                    "passing-oracle"
                })
            );
        }
    }
    #[test]
    fn clipped_run_accepts_whole_table_but_not_clipped_upload() {
        let mut a = audit();
        let before = a.mark;
        // Only one glyph of the run is visible: upload its whole source table once.
        upload(&mut a, FONT, abi::purpose::FIXTURE, FONT.len() as u32);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), None);
        assert_eq!(a.hashes, std::vec![(sha256(FONT), true)]);
        let before = a.mark;
        upload(&mut a, FONT, abi::purpose::FIXTURE, 7);
        assert_eq!(a.refusal(before, 0, 7), Some("passing-oracle"));
    }
    #[test]
    fn table_outside_the_program_image_is_refused() {
        let mut a = Audit { active: true, ..Audit::default() };
        let leaked: &'static [u8] = std::vec![14, 17, 17, 31].leak();
        // Only FONT is in the (simulated) read-only image; the leaked copy has its bytes.
        let image = |r: &[u8]| core::ptr::eq(r, FONT);
        a.declare(std::vec![FONT], image);
        assert_eq!(a.refusal(a.mark, 0, 0), None);
        a.declare(std::vec![FONT, leaked], image);
        assert_eq!(a.tables.len(), 1, "a table with a leaked row is not declared");
        assert_eq!(a.refusal(a.mark, 0, 0), Some("non-static-table"));
    }
    #[test]
    fn declared_table_licenses_one_upload() {
        let mut a = audit();
        let before = a.mark;
        upload(&mut a, FONT, abi::purpose::FIXTURE, FONT.len() as u32);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), None);
        // The same bytes again, as a CPU fill repeating a declared row would upload them.
        let before = a.mark;
        upload(&mut a, FONT, abi::purpose::FIXTURE, FONT.len() as u32);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), Some("passing-oracle"));
        // A fresh declaration licenses a fresh upload.
        a.tables.push(Table { rows: std::vec![FONT] });
        let before = a.mark;
        upload(&mut a, FONT, abi::purpose::FIXTURE, FONT.len() as u32);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), None);
    }
    #[test]
    fn buffer_uploads_are_content_checked() {
        let mut a = audit();
        let mut r = abi::ResourceCreate {
            handle: 2,
            kind: abi::kind::BUFFER,
            size: 64,
            ..abi::ResourceCreate::ZERO
        };
        let crossings = |a: &mut Audit, bytes: &[u8]| {
            let mut u = abi::Upload {
                resource: 2,
                width: bytes.len() as u32,
                height: 1,
                pixels: bytes.as_ptr() as u64,
                pixels_len: bytes.len() as u64,
                stride: bytes.len() as u32,
                source: abi::upload_source::R8,
                purpose: abi::purpose::FIXTURE,
                ..abi::Upload::ZERO
            };
            unsafe { observe_in(a, abi::op::UPLOAD, &mut u as *mut _ as *mut u8, core::mem::size_of_val(&u)) }
        };
        unsafe { observe_in(&mut a, abi::op::RESOURCE_CREATE, &mut r as *mut _ as *mut u8, core::mem::size_of_val(&r)) };
        // Words a CPU computed (here, per-pixel coverage spans) are not a declared table.
        let computed = [0u8, 0, 0, 0, 0, 0, 128, 63, 1, 0, 0, 0];
        let before = a.mark;
        crossings(&mut a, &computed);
        assert_eq!(a.refusal(before, 0, computed.len() as u64), Some("passing-oracle"));
        let before = a.mark;
        crossings(&mut a, FONT);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), None, "a declared static table may fill a buffer");
    }
    #[test]
    fn once_per_context_table_upload_allows_later_runs_without_uploads() {
        let mut a = audit();
        let before = a.mark;
        upload(&mut a, FONT, abi::purpose::FIXTURE, FONT.len() as u32);
        assert_eq!(a.refusal(before, 0, FONT.len() as u64), None);
        let before = a.mark;
        // Cached or offscreen runs need no UPLOAD.
        assert_eq!(a.refusal(before, 0, 0), None);
        assert_eq!(a.hashes.len(), 1);
        // A bypassed successful syscall is still refused by kernel-counter reconciliation.
        assert_eq!(a.refusal(before, 0, 14), Some("unobserved-upload"));
    }
}
