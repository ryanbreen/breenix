//! Host syscall boundary: resources and the production GPU stream validator.
use crate::gpu::abi::{
    self, op,
    stream::{self, Resolver, SourceInfo, TargetInfo},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::atomic::{AtomicU32, Ordering},
};
pub static SCALE: AtomicU32 = AtomicU32::new(1);
#[derive(Default)]
struct State {
    next: u64,
    resources: BTreeMap<u64, abi::ResourceCreate>,
}
thread_local! { static STATE:RefCell<State>=RefCell::default(); }
impl Resolver for State {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        let r = self.resources.get(&h)?;
        Some(TargetInfo {
            width: r.width,
            height: r.height,
            invalid: false,
            format: r.format,
        })
    }
    fn source(&self, h: u64) -> Option<SourceInfo> {
        let r = self.resources.get(&h)?;
        Some(SourceInfo {
            width: r.width,
            height: r.height,
            invalid: false,
            format: r.format,
        })
    }
    fn buffer_size(&self, h: u64) -> Option<u64> {
        self.resources.get(&h).map(|r| r.size)
    }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] {
        [0; 4]
    }
}
pub unsafe fn dispatch(code: u32, p: u64) -> i64 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        match code {
            op::CURRENT_MODE => {
                *(p as *mut abi::Mode) = abi::Mode {
                    width: 2560,
                    height: 1600,
                    scale: SCALE.load(Ordering::Relaxed),
                    reserved: 0,
                };
                0
            }
            op::MODES => {
                let m = &mut *(p as *mut abi::Modes);
                m.count = 1;
                m.modes[0] = abi::Mode {
                    width: 2560,
                    height: 1600,
                    scale: SCALE.load(Ordering::Relaxed),
                    reserved: 0,
                };
                0
            }
            op::CONTEXT_CREATE => {
                s.next += 1;
                (*(p as *mut abi::ContextCreate)).handle = s.next;
                0
            }
            op::RESOURCE_CREATE => {
                s.next += 1;
                let r = &mut *(p as *mut abi::ResourceCreate);
                r.handle = s.next;
                s.resources.insert(r.handle, *r);
                0
            }
            op::RESOURCE_DESTROY => {
                s.resources.remove(&(*(p as *mut abi::Handle)).handle);
                0
            }
            op::CONTEXT_DESTROY | op::UPLOAD => 0,
            op::WAIT => {
                (*(p as *mut abi::Wait)).status = abi::fence_status::OK;
                0
            }
            op::SUBMIT => {
                let a = &mut *(p as *mut abi::Submit);
                let words =
                    std::slice::from_raw_parts(a.stream as *const u32, a.length as usize / 4);
                match stream::validate(words, &*s) {
                    Ok(_) => {
                        a.fence = 1;
                        0
                    }
                    Err(r) => {
                        eprintln!(
                            "GPU admission refused command {} with stream error {}",
                            r.index, r.error
                        );
                        if r.is_work_limit() {
                            -122
                        } else {
                            -22
                        }
                    }
                }
            }
            _ => panic!("unexpected op {code}"),
        }
    })
}
