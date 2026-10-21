//! windows: window lifecycle on every target (contract section 17).

use crate::*;

pub(crate) const WINDOWS: &[Case] = &[
    pictured!("create", "Window creation returns distinct zeroed GPU target handles and mapped tree pages", windows_create),
    pictured!("create-zeroed", "A new window's buffers are all zero", windows_create_zeroed),
    pictured!("register-listed", "A registered window is listed with its title, size and owner", windows_register_listed),
    pictured!("title-limit", "A 64-byte title is kept whole, and a longer one is refused with EINVAL", windows_title_limit),
    pictured!("title-utf8", "A UTF-8 title is stored and listed byte for byte", windows_title_utf8),
    pictured!("title-change", "Setting a new title updates the listing", windows_title_change),
    pictured!("size-zero", "A zero width or height fails with EINVAL", windows_size_zero),
    pictured!("size-too-big", "A dimension over 4,096 fails with EINVAL", windows_size_too_big),
    pictured!("limit", "The seventeenth window fails with the contract's errno, and the first sixteen are unaffected", windows_limit),
    pictured!("multi-window", "One process creates and draws four windows independently", windows_multi_window),
    pictured!("destroy", "Destroy removes the window, revokes GPU target handles and releases backing", windows_destroy),
    pictured!("drop-destroys", "Dropping a Breengel Window destroys it", windows_drop_destroys),
    pictured!("stale-id", "Every op on a destroyed window's id fails with ENOENT, and ids are never reused in a boot", windows_stale_id),
    pictured!("register-unwinds", "When registration fails, Window::new frees the buffers it created", windows_register_unwinds),
    pictured!("exit-cleanup", "A process's windows disappear when it exits", windows_exit_cleanup),
    pictured!("exec-cleanup", "A process's windows disappear when it execs", windows_exec_cleanup),
    pictured!("fork-no-mapping", "A forked child inherits no window GPU handles", windows_fork_no_mapping),
    pictured!("fork-no-authority", "A forked child cannot commit, resize or destroy its parent's window", windows_fork_no_authority),
    pictured!("commit-swaps", "After a commit, acquire returns a different buffer holding the committed pixels", windows_commit_swaps),
    pictured!("acquire-replays-damage", "With three buffers and alternating disjoint updates (A commits `[1,0]`, B commits `[1,2]` with only the second cell damaged), the next acquired buffer holds `[1,2]`, and every acquire over 300 such commits equals the newest front", windows_acquire_replays_damage),
    pictured!("acquire-full-copy", "A buffer with no base, or whose missing commits have left the 8-entry damage log, is replayed in full by the kernel and equals the newest front, for a Breengel client and a raw client alike", windows_acquire_full_copy),
    pictured!("stale-base-commit", "Of two concurrently acquired buffers, committing the one with the older base gives exactly that buffer's pixels, with damage widened to cover every pixel that changed. With `acquire-stall` holding thread 1's replay copy from front A halfway, thread 2 commits C and then acquires: it does not get A until the copy ends, and thread 1's buffer equals exactly the commit its returned base names. A stale buffer committed after more than eight intervening commits gets whole-window damage. Every step compares whole pixels to the formula and checks that the damage covers every changed pixel", windows_stale_base_commit),
    pictured!("commit-immutable", "Committed GPU targets refuse later writes while accepted renders complete and tree pages stay protected", windows_commit_immutable),
    pictured!("commit-busy", "Committing a buffer twice without acquiring it again fails with EBUSY", windows_commit_busy),
    pictured!("thread-commit", "Two threads of the owner committing concurrently both succeed with distinct, ordered commit ids", windows_thread_commit),
    pictured!("release-order", "A retired buffer returns to free only after its composite and any capture copy in progress are done", windows_release_order),
    pictured!("acquire-timeout", "acquire with no free buffer returns ETIMEDOUT no earlier than its timeout and within 100 ms after it", windows_acquire_timeout),
    pictured!("resize", "The owner's resize gives buffers of the new size holding the old contents in the intersection, also while another thread of the owner resizes the window", windows_resize),
    pictured!("resize-error", "A resize the kernel refuses is reported as an error, and no Resized event claims the new size", windows_resize_error),
    pictured!("resize-rlimit", "A resize over RLIMIT_AS fails with ENOMEM and leaves the old buffers intact", windows_resize_rlimit),
    pictured!("resize-rollback", "A failure at each allocation step of create and resize leaves registry, mappings and accounting unchanged", windows_resize_rollback),
    pictured!("rlimit-accounting", "Window guest backing counts against RLIMIT_AS and the GPU quota and destruction releases both charges", windows_rlimit_accounting),
    pictured!("capture-across-destroy", "A capture in progress when its window is destroyed or resized returns the whole commit, with the tree page of that same commit when a tree destination is given; a capture begun after the destroy is ENOENT; and the buffer is freed when the copy ends", windows_capture_across_destroy),
    pictured!("two-buffer-capture-load", "A two-buffer client committing 300 frames while a reader captures it continuously and an evidence holder is registered never waits in acquire beyond one composite and one capture copy", windows_two_buffer_capture_load),
    pictured!("remnant-on-death", "When the owner of a window with an evidence holder is killed, the holder reads its last front and tree, and releasing the remnant returns memory to the baseline", windows_remnant_on_death),
    pictured!("churn", "100 create and destroy cycles return the registry, allocation counter and memory to the baseline", windows_churn),
    pictured!("distinct-content", "Sixteen live windows each composite their own pixels, never another window's", windows_distinct_content),
    pictured!("raw-client", "A client using the raw libbreenix calls, without Breengel, creates, commits and destroys a window", windows_raw_client),
    pictured!("every-target", "Window creation works on this target's backend and reports the backend by name", windows_every_target),
    pictured!("commit-waits-render", "Pending render fences are not composed and failed renders cannot replace the front", windows_commit_waits_render),
];

// ---------------------------------------------------------------------------------------
// windows: window lifecycle on every target (contract section 17)
// ---------------------------------------------------------------------------------------

use super::kit::*;
use abi::op;
use abi::Plain;

/// Window targets are BGRX: canonicalize X before comparing the colour bytes (B11).
fn window_pixels(pixels: &[u8], width: u32, height: u32, what: &str,
    section: &str, expected: impl Fn(u32, u32) -> [u8; 4]) -> CaseResult {
    let mut canonical = pixels.to_vec();
    for pixel in canonical.chunks_exact_mut(4) { pixel[3] = 0xFF; }
    same(&canonical, width, height, true, what, section, |x, y| {
        let mut pixel = expected(x, y);
        pixel[3] = 0xFF;
        pixel
    })
}

// gpu-window: §2.2 CREATE returns GPU target handles; zeroing, count, identity and tree metadata remain guarded.
fn windows_create() -> CaseResult {
interface()?;
    for buffers in [2u32,3] {
        let w=W::new(64,48,buffers)?;
        ensure(w.id()>=1 && w.c.stride==256,"17.1","gpu-window: CREATE returned invalid identity or stride")?;
        let mut handles=std::collections::BTreeSet::new();
        for i in 0..3 {
            let target=w.c.buffer[i];
            if i>=buffers as usize {ensure(target==0,"17.1","gpu-window: CREATE published a target past its buffer count")?;continue;}
            ensure(target!=0 && handles.insert(target),"17.1","gpu-window: CREATE returned zero or repeated GPU handles")?;
            let mut px=vec![0xA5;64*48*4];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
            let r=libbreenix::gpu::readback(target,[0,0,64,48],&mut px,256,&mut record);
            want_ok(r,"17.1","gpu-window: readback of CREATE target")?;
            window_pixels(&px,64,48,"new target","17.2",|_,_|[0;4])?;
        }
        ensure(w.c.tree!=0 && w.c.tree_kib==64,"17.1","gpu-window: CREATE lost its separately mapped tree pages")?;
    }Ok(())
}

fn windows_create_zeroed() -> CaseResult {
    interface()?;
    let w = W::new(37, 29, 3)?;
    for i in 0..3 {
        let mut pixels = vec![0xA5;37*29*4];
        let mut record = [0;libbreenix::gpu::abi::record::SIZE];
        want_ok(libbreenix::gpu::readback(w.c.buffer[i as usize],[0,0,37,29],&mut pixels,37*4,&mut record),
            "17.2","new buffer device readback")?;
        window_pixels(&pixels,37,29,&format!("new buffer {i}"),"17.2",|_,_|[0;4])?;
        if let Some(pos) = w.tree(i).iter().position(|&b| b != 0) {
            return at("17.2", format!("tree page {i} of a new window holds a nonzero byte at {pos}"));
        }
    }
    Ok(())
}

fn windows_register_listed() -> CaseResult {
    interface()?;
    let w = W::new(33, 21, 2)?;
    ensure(listed(w.id()).cite("17.1")?.is_none(), "17.2", "LIST shows a window that was never registered; it lists registered windows only")?;
    want_ok(w.register(b"register-listed"), "17.1", "REGISTER")?;
    let info = listed(w.id()).cite("17.1")?.ok_or("LIST does not show the window just registered (section 17.2)")?;
    ensure(&info.title[..info.title_len as usize] == b"register-listed", "17.2", format!("LIST gives the title {:?}", String::from_utf8_lossy(&info.title[..info.title_len.min(64) as usize])))?;
    ensure((info.width, info.height) == (33, 21), "17.1", format!("LIST gives the size {}x{}; it was created 33x21", info.width, info.height))?;
    ensure(info.owner as u64 == getpid(), "17.1", format!("LIST gives the owner pid {}; this process is {}", info.owner, getpid()))?;
    ensure(info.flags & abi::win_flags::REGISTERED != 0, "17.1", format!("LIST gives flags {:#x} without the registered bit", info.flags))
}

fn title_of(id: u64) -> Result<Vec<u8>, String> {
    let info = listed(id)?.ok_or_else(|| String::from("the window is not listed"))?;
    Ok(info.title[..(info.title_len as usize).min(64)].to_vec())
}

fn windows_title_limit() -> CaseResult {
    interface()?;
    let w = W::new(8, 8, 2)?;
    let t64: Vec<u8> = (0..64).map(|i| b'a' + (i % 26) as u8).collect();
    want_ok(w.register(&t64), "17.2", "REGISTER with a 64-byte title")?;
    ensure(title_of(w.id()).cite("17.2")? == t64, "17.2", "LIST does not give the 64-byte title whole")?;
    let t65: Vec<u8> = (0..65).map(|i| b'A' + (i % 26) as u8).collect();
    want_errno(title_op(op::SET_TITLE, w.id(), t65.as_ptr() as u64, 65), EINVAL, "17.2", "SET_TITLE with a 65-byte title")?;
    want_errno(w.register(&t65), EINVAL, "17.2", "REGISTER with a 65-byte title")?;
    ensure(title_of(w.id()).cite("17.2")? == t64, "17.2", "a refused 65-byte title changed the listed title")
}

fn windows_title_utf8() -> CaseResult {
    interface()?;
    let w = W::new(8, 8, 2)?;
    let title = "fenêtre ⊞ 窓 🪟".as_bytes();
    want_ok(w.register(title), "17.2", "REGISTER with a UTF-8 title")?;
    ensure(title_of(w.id()).cite("17.2")? == title, "17.2", "LIST does not give the UTF-8 title byte for byte")?;
    let bad = [b'o', b'k', 0xC3, 0x28];
    want_errno(title_op(op::SET_TITLE, w.id(), bad.as_ptr() as u64, 4), EINVAL, "17.2", "SET_TITLE with invalid UTF-8")?;
    ensure(title_of(w.id()).cite("17.2")? == title, "17.2", "a refused invalid title changed the listed title")
}

fn windows_title_change() -> CaseResult {
    interface()?;
    let w = W::new(8, 8, 2)?;
    want_ok(w.register(b"first"), "17.1", "REGISTER")?;
    want_ok(title_op(op::SET_TITLE, w.id(), b"second".as_ptr() as u64, 6), "17.1", "SET_TITLE")?;
    let got = title_of(w.id()).cite("17.1")?;
    ensure(got == b"second", "17.1", format!("after SET_TITLE \"second\" LIST gives {:?}", String::from_utf8_lossy(&got)))
}

fn create_errno(width: u32, height: u32) -> i64 {
    let mut c = abi::Create::ZERO;
    c.width = width;
    c.height = height;
    c.buffers = 2;
    let r = call(op::CREATE, &mut c);
    if r >= 0 {
        let _ = call_id(op::DESTROY, c.id);
    }
    r
}

fn windows_size_zero() -> CaseResult {
    interface()?;
    for (w, h) in [(0, 10), (10, 0), (0, 0)] {
        want_errno(create_errno(w, h), EINVAL, "17.2", &format!("CREATE {w}x{h}"))?;
    }
    Ok(())
}

fn windows_size_too_big() -> CaseResult {
    interface()?;
    for (w, h) in [(4097, 10), (10, 4097), (u32::MAX, 1)] {
        want_errno(create_errno(w, h), EINVAL, "17.2", &format!("CREATE {w}x{h}"))?;
    }
    want_ok(create_errno(4096, 1), "17.2", "CREATE 4096x1, the largest width")?;
    want_ok(create_errno(1, 4096), "17.2", "CREATE 1x4096, the largest height").map(|_| ())
}

fn windows_limit() -> CaseResult {
    interface()?;
    let mut wins = Vec::new();
    for i in 0..16 {
        wins.push(W::new(8, 8, 2).map_err(|_| format!("CREATE of window {} of 16 failed; a process may own 16 (section 17.2)", i + 1))?);
    }
    want_errno(create_errno(8, 8), EAGAIN, "17.2", "the seventeenth CREATE")?;
    for (i, w) in wins.iter().enumerate() {
        w.frame(i as u32).map_err(|e| format!("window {} of the first sixteen after the refusal: {e} (section 17.2)", i + 1))?;
    }
    Ok(())
}

fn windows_multi_window() -> CaseResult {
    interface()?;
    let sizes = [(40u32, 30u32), (17, 23), (64, 8), (9, 50)];
    let wins: Vec<W> = sizes.iter().map(|&(w, h)| W::new(w, h, 2)).collect::<Result<_, _>>()?;
    for (i, w) in wins.iter().enumerate() {
        w.frame(100 + i as u32).cite("17.3")?;
    }
    for (i, w) in wins.iter().enumerate() {
        let (c, px) = capture(w.id(), w.c.width, w.c.height, None).map_err(|r| format!("CAPTURE of window {i} {}", outcome(r))).cite("17.3")?;
        ensure((c.width, c.height) == (w.c.width, w.c.height), "17.3", format!("CAPTURE of window {i} reports {}x{}", c.width, c.height))?;
        same(&px, w.c.width, w.c.height, false, &format!("window {i}"), "17.3", |x, y| pattern(100 + i as u32, x, y))?;
    }
    Ok(())
}

/// Establish that CREATE returned live, readable GPU targets before testing revocation.
fn readable_window_targets(w: &W) -> CaseResult {
    for target in &w.c.buffer[..w.c.buffers as usize] {
        let mut pixels = vec![0xA5; w.len()];
        let mut record = [0; libbreenix::gpu::abi::record::SIZE];
        want_ok(libbreenix::gpu::readback(*target, [0,0,w.c.width,w.c.height], &mut pixels,
            w.c.width*4, &mut record), "17.2", "gpu-window: live target READBACK before revocation")?;
        window_pixels(&pixels,w.c.width,w.c.height,"new live target","17.2",|_,_|[0;4])?;
    }
    Ok(())
}

// gpu-window: §2.2 replaces unmapped pixel addresses with revoked target handles and exact GPU accounting.
fn windows_destroy() -> CaseResult {
interface()?;
    let before=crate::gpu_checks::counters().cite("17.2")?;
    let mut w=W::new(64,48,2)?;let id=w.id();let handles=w.c.buffer;
    want_ok(w.register(b"destroy"),"17.2","REGISTER before DESTROY")?;
    ensure(listed(id).cite("17.2")?.is_some(),"17.2","window was not listed before DESTROY")?;
    readable_window_targets(&w)?;
    want_ok(w.destroy(),"17.2","gpu-window: DESTROY")?;
    ensure(listed(id).cite("17.2")?.is_none(),"17.2","gpu-window: destroyed window remains listed")?;
    for handle in &handles[..2] {
        let mut pixels=vec![0xA5;64*48*4];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
        want_errno(libbreenix::gpu::readback(*handle,[0,0,64,48],&mut pixels,256,&mut record),EINVAL,"17.2","gpu-window: destroyed GPU handle")?;
        ensure(pixels.iter().all(|&b|b==0xA5),"17.2","gpu-window: revoked handle returned pixels")?;
    }
    let start=now_ms();loop {
        let after=crate::gpu_checks::counters().cite("17.2")?;
        if after.resources==before.resources && after.resource_bytes==before.resource_bytes {break;}
        ensure(since(start)<1_000,"17.2","gpu-window: DESTROY leaked target handles or backing")?;
        let _=time::sleep_ms(5);
    }Ok(())
}

fn windows_drop_destroys() -> CaseResult {
    interface()?;
    let id = {
        let w = breengel::Window::new_without_fonts(b"drop-destroys", 24, 24).cite("17.2")?;
        let id = w.id();
        ensure(listed(id).cite("17.2")?.is_some(), "17.2", "a Breengel Window is not listed while it lives")?;
        id
    };
    ensure(listed(id).cite("17.2")?.is_none(), "17.2", "after the Breengel Window was dropped its window is still listed")?;
    want_errno(call_id(op::DESTROY, id), ENOENT, "17.2", "DESTROY of the dropped Window's id")
}

// gpu-window: B7 replaces retired MAP_COMMITTED with GPU-buffer acquisition; stale ids remain ENOENT.
fn windows_stale_id() -> CaseResult {
    interface()?;
    let mut w = W::new(8, 8, 2)?;
    let id = w.id();
    want_ok(w.destroy(), "17.2", "DESTROY")?;
    let mut fails = Vec::new();
    let mut check = |name: &str, r: i64| if r != -(ENOENT as i64) { fails.push(format!("{name} {}", outcome(r))) };
    check("DESTROY", call_id(op::DESTROY, id));
    check("REGISTER", title_op(op::REGISTER, id, b"x".as_ptr() as u64, 1));
    check("SET_TITLE", title_op(op::SET_TITLE, id, b"x".as_ptr() as u64, 1));
    let mut a = abi::Acquire::ZERO;
    a.id = id;
    check("ACQUIRE", call(op::ACQUIRE, &mut a));
    let mut c = abi::Commit::ZERO;
    c.id = id;
    check("COMMIT", call(op::COMMIT, &mut c));
    let mut n = abi::NoChange::ZERO;
    n.id = id;
    check("COMMIT_NO_CHANGE", call(op::COMMIT_NO_CHANGE, &mut n));
    let mut r = abi::Resize::ZERO;
    r.id = id;
    r.width = 4;
    r.height = 4;
    check("RESIZE", call(op::RESIZE, &mut r));
    let mut ev = [abi::WindowEvent::ZERO; 1];
    let mut re = abi::ReadEvents::ZERO;
    re.id = id;
    re.ptr = ev.as_mut_ptr() as u64;
    re.capacity = 1;
    check("READ_EVENTS", call(op::READ_EVENTS, &mut re));
    check("EVENT_FD", call_id(op::EVENT_FD, id));
    // ACQUIRE is the GPU target handoff; a refused stale id must publish no buffer.
    let mut stale = abi::Acquire::ZERO;
    stale.id = id;
    stale.buffer = u32::MAX;
    check("GPU ACQUIRE", call(op::ACQUIRE, &mut stale));
    ensure(stale.buffer == u32::MAX && stale.base == 0, "17.2",
        "gpu-window: a stale id published an acquired GPU buffer or base")?;
    check("CAPTURE", capture(id, 8, 8, None).err().unwrap_or(0));
    let mut t = abi::MapTree::ZERO;
    t.id = id;
    check("MAP_TREE", call(op::MAP_TREE, &mut t));
    let mut meta = abi::Meta::ZERO;
    let mut q = abi::MetaQuery { id, meta: &mut meta as *mut abi::Meta as u64, commit_id: 0 };
    check("READ_META", call(op::READ_META, &mut q));
    ensure(fails.is_empty(), "17.2", format!("on a destroyed window's id: {}; every op is ENOENT", fails.join(", ")))?;
    let mut last = id;
    for _ in 0..5 {
        let w2 = W::new(8, 8, 2)?;
        ensure(w2.id() > last, "17.2", format!("CREATE after window {last} gave id {}; ids are never reused in a boot", w2.id()))?;
        last = w2.id();
    }
    Ok(())
}

fn windows_register_unwinds() -> CaseResult {
    interface()?;
    let probe = W::new(8, 8, 2)?;
    let before = probe.id();
    let long = [b't'; 65];
    let result = breengel::Window::new_without_fonts(&long, 16, 16);
    ensure(result.is_err(), "17.2", "Breengel's Window::new succeeded with a 65-byte title; registration must refuse it")?;
    // Window::new created window `before + 1` (ids rise by one per CREATE in this case's
    // boot-wide sequence) and must have destroyed it.
    let after = W::new(8, 8, 2)?;
    ensure(after.id() == before + 2, "17.2", format!(
        "windows {before} and {} bracket Window::new, which should have created exactly one window in between", after.id()))?;
    want_errno(call_id(op::DESTROY, before + 1), ENOENT, "17.2",
        "DESTROY of the window Window::new created before its registration failed")
}

/// Window ids reported by a child on its output, one per line, then the child waits.
fn ids_from(h: &mut Helper, n: usize) -> Result<Vec<u64>, String> {
    (0..n).map(|_| h.line(5_000)?.parse::<u64>().map_err(|e| format!("the child sent a bad id: {e}"))).collect()
}

fn windows_exit_cleanup() -> CaseResult {
    interface()?;
    let mut body = || {
        for t in [&b"exit-a"[..], b"exit-b"] {
            let Ok(w) = W::new(16, 16, 2) else { return 2 };
            if w.register(t) < 0 { return 3; }
            let _ = io::write(libbreenix::types::Fd::STDOUT, format!("{}\n", w.id()).as_bytes());
            core::mem::forget(w);
        }
        0
    };
    let mut h = Helper::call(&mut body).cite("17.2")?;
    let ids = ids_from(&mut h, 2).cite("17.2")?;
    let mut status = 0;
    let _ = process::waitpid(h.pid, &mut status, 0);
    h.pid = 0;
    for id in ids {
        ensure(listed(id).cite("17.2")?.is_none(), "17.2", format!("window {id} is still listed after its owner exited"))?;
        want_errno(capture(id, 16, 16, None).err().unwrap_or(0), ENOENT, "17.2", &format!("CAPTURE of window {id} after its owner exited"))?;
    }
    Ok(())
}

fn windows_exec_cleanup() -> CaseResult {
    interface()?;
    let mut body = || {
        let Ok(w) = W::new(16, 16, 2) else { return 2 };
        if w.register(b"exec-cleanup") < 0 { return 3; }
        let _ = io::write(libbreenix::types::Fd::STDOUT, format!("{}\n", w.id()).as_bytes());
        core::mem::forget(w);
        let argv = [b"raw-window_test\0".as_ptr(), core::ptr::null()];
        let _ = process::execv(b"/usr/local/test/bin/raw-window_test\0", argv.as_ptr());
        4
    };
    let mut h = Helper::call(&mut body).cite("17.2")?;
    let id = ids_from(&mut h, 1).cite("17.2")?[0];
    let ready = h.line(5_000).cite("17.2")?;
    ensure(ready == "ready", "17.2", format!("the exec'd program said `{ready}`"))?;
    ensure(listed(id).cite("17.2")?.is_none(), "17.2", format!("window {id} is still listed after its owner exec'd"))?;
    let reply = h.ask(&format!("destroy {id}")).cite("17.2")?;
    ensure(reply == "err 2", "17.2", format!("the exec'd image's DESTROY of its old window {id} answered `{reply}`; the window is gone (ENOENT)"))
}

// gpu-window: §2.2 fork-no-mapping becomes no inherited GPU handles, with an actual forked child.
fn windows_fork_no_mapping() -> CaseResult {
interface()?;
    let w=W::new(16,16,2)?;let handles=w.c.buffer;
    readable_window_targets(&w)?;
    step("17.2",child("GPU handles after fork",3_000,None,|| {
        let (ret,c)=libbreenix::gpu::counters(0);
        if ret!=0 || c.resources!=0 || c.contexts!=0 {return Err("gpu-window: fork inherited GPU resources or contexts".into());}
        for target in &handles[..2] {
            let mut pixels=vec![0xA5;16*16*4];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
            if libbreenix::gpu::readback(*target,[0,0,16,16],&mut pixels,64,&mut record)!=-(EINVAL as i64){return Err("gpu-window: child read its parent's GPU target instead of EINVAL".into());}
            if pixels.iter().any(|&b|b!=0xA5){return Err("gpu-window: refused inherited target wrote pixels".into());}
        }Ok(())
    }))
}

fn windows_fork_no_authority() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let (id, buffer) = (w.id(), a.buffer);
    child("the forked child", 5_000, None, move || {
        let mut fails = Vec::new();
        let r = commit(id, buffer, &[], None).err().unwrap_or(0);
        if r != -(EPERM as i64) { fails.push(format!("COMMIT {}", outcome(r))); }
        let mut rs = abi::Resize::ZERO;
        rs.id = id;
        rs.width = 8;
        rs.height = 8;
        let r = call(op::RESIZE, &mut rs);
        if r != -(EPERM as i64) { fails.push(format!("RESIZE {}", outcome(r))); }
        let r = call_id(op::DESTROY, id);
        if r != -(EPERM as i64) { fails.push(format!("DESTROY {}", outcome(r))); }
        let r = acquire(id, 0).err().unwrap_or(0);
        if r != -(EPERM as i64) { fails.push(format!("ACQUIRE {}", outcome(r))); }
        if fails.is_empty() { Ok(()) } else { Err(format!("the forked child's {}; the contract gives EPERM (section 17.4)", fails.join(", "))) }
    })?;
    w.commit(buffer, &[full(&w.c)]).map_err(|r| format!("the parent's own COMMIT after the child's attempts {}", outcome(r))).cite("17.4")?;
    Ok(())
}

fn windows_commit_swaps() -> CaseResult {
    interface()?;
    let w = W::new(20, 10, 2)?;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    fill(w.buf(a.buffer), 20, 10, 5);
    let c = w.commit(a.buffer, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
    let b = w.acquire(1_000_000_000).map_err(|r| format!("the second ACQUIRE {}", outcome(r))).cite("17.3")?;
    ensure(b.buffer != a.buffer, "17.3", format!("after committing buffer {} ACQUIRE returned it again; the committed buffer is the front", a.buffer))?;
    ensure(b.base == c.commit_id, "17.3", format!("ACQUIRE gave base {}; the buffer holds commit {}", b.base, c.commit_id))?;
    same(w.buf(b.buffer), 20, 10, false, "the acquired buffer", "17.3", |x, y| pattern(5, x, y))
}

/// Two-cell windows: cell 0 is x < 32, cell 1 is x >= 32, of a 64x16 window.
const CELL_W: u32 = 32;

fn cell(i: u32) -> abi::Rect {
    abi::Rect::new((i * CELL_W) as i32, 0, CELL_W as i32, 16)
}

fn cells_expect(c0: u32, c1: u32) -> impl Fn(u32, u32) -> [u8; 4] {
    move |x, y| pattern(if x < CELL_W { c0 } else { c1 }, x, y)
}

fn windows_acquire_replays_damage() -> CaseResult {
    interface()?;
    let w = W::new(64, 16, 3)?;
    // Commit [1, 0]: cell 0 seed 1, cell 1 seed 0.
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    fill_rect(w.buf(a.buffer), 64, cell(0), 1);
    fill_rect(w.buf(a.buffer), 64, cell(1), 0);
    w.commit(a.buffer, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
    let mut front = (1u32, 0u32);
    for k in 0..300u32 {
        let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {k} {}", outcome(r))).cite("17.3")?;
        let (c0, c1) = front;
        same(w.buf(a.buffer), 64, 16, false, &format!("acquire {k} (buffer {}, base {})", a.buffer, a.base), "17.3", cells_expect(c0, c1))?;
        // Update one cell, alternating, and damage only that cell.
        let which = k % 2 + if k == 0 { 1 } else { 0 };
        let which = which % 2;
        let seed = 2 + k;
        fill_rect(w.buf(a.buffer), 64, cell(which), seed);
        w.commit(a.buffer, &[cell(which)]).map_err(|r| format!("COMMIT {k} {}", outcome(r))).cite("17.3")?;
        front = if which == 0 { (seed, c1) } else { (c0, seed) };
    }
    Ok(())
}

/// The frame a window's newest commit shows, by the seed this case drew it with.
struct Front {
    seed: u32,
}

/// Acquire, check the buffer equals the newest front, and note whether the kernel replayed a
/// buffer more than eight pixel commits behind in full. ACQUIRE returns the buffer's base after
/// its replay (the newest front), so the base it had before is tracked here: `holds[b]` is the
/// commit buffer `b` held when it was last committed or replayed, 0 for none.
fn acquire_checked(w: &W, front: &Front, newest: u64, holds: &mut [u64; 3], seen_overflow: &mut bool) -> Result<abi::Acquire, libbreenix::suite::CaseError> {
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    same(w.buf(a.buffer), w.c.width, w.c.height, false, &format!("buffer {} acquired at base {} (newest commit {newest})", a.buffer, a.base), "17.3", |x, y| pattern(front.seed, x, y))?;
    ensure(a.base == newest, "17.3", format!("ACQUIRE returned buffer {} with base {}; after its replay it holds the newest commit {newest}", a.buffer, a.base))?;
    let before = holds[a.buffer as usize];
    if before != 0 && before + 8 < newest {
        ensure(a.replay_full == 1, "17.3", format!("buffer {} held commit {before}, more than eight commits behind commit {newest}, and was replayed in {} rectangles, not in full", a.buffer, a.replay_count))?;
        *seen_overflow = true;
    }
    holds[a.buffer as usize] = a.base;
    Ok(a)
}

fn windows_acquire_full_copy() -> CaseResult {
    interface()?;
    // A buffer with no base: the second buffer acquired after the first commit.
    let w = W::new(48, 12, 3)?;
    let mut newest = w.frame(31).cite("17.3")?.commit_id;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    ensure(a.base == newest && a.replay_full == 1, "17.3", format!(
        "ACQUIRE of a buffer that never held a commit gave base {} with {} rectangles; it is replayed in full to commit {newest}", a.base, a.replay_count))?;
    same(w.buf(a.buffer), 48, 12, false, "a buffer with no base after its replay", "17.3", |x, y| pattern(31, x, y))?;
    let mut front = Front { seed: 32 };
    fill(w.buf(a.buffer), 48, 12, 32);
    newest = w.commit(a.buffer, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?.commit_id;
    let mut holds = [0u64; 3];
    holds[a.buffer as usize] = newest;
    // Give every buffer a commit by holding two at a time, then commit one at a time: the
    // free buffer ACQUIRE leaves unused falls behind. Ten single commits later, acquiring two
    // at once reaches it.
    let mut seen = false;
    let mut based = std::collections::BTreeSet::new();
    for _ in 0..6 {
        let x = acquire_checked(&w, &front, newest, &mut holds, &mut seen)?;
        let y = acquire_checked(&w, &front, newest, &mut holds, &mut seen)?;
        for (b, s) in [(x.buffer, front.seed + 1), (y.buffer, front.seed + 2)] {
            fill(w.buf(b), 48, 12, s);
            newest = w.commit(b, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?.commit_id;
            holds[b as usize] = newest;
            based.insert(b);
        }
        front.seed += 2;
    }
    ensure(based.len() == 3, "17.3", format!("holding two buffers at a time committed only buffers {based:?} of three"))?;
    for _ in 0..10 {
        let x = acquire_checked(&w, &front, newest, &mut holds, &mut seen)?;
        front.seed += 1;
        fill(w.buf(x.buffer), 48, 12, front.seed);
        newest = w.commit(x.buffer, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?.commit_id;
        holds[x.buffer as usize] = newest;
    }
    let _first = acquire_checked(&w, &front, newest, &mut holds, &mut seen)?;
    let _second = acquire_checked(&w, &front, newest, &mut holds, &mut seen)?;
    ensure(seen, "17.3", "no ACQUIRE handed out a buffer more than eight commits behind, so the log-overflow replay was not reached")?;
    // The same through a raw client: its buffer with no base after a commit.
    let mut r = raw_client().cite("24")?;
    let created = r.ask("create 48 12 3").cite("24")?;
    let id: u64 = created.strip_prefix("ok ").and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("raw-window_test create answered `{created}`")).cite("24")?;
    let filled = r.ask(&format!("fill {id} 77")).cite("17.3")?;
    ensure(filled.starts_with("ok "), "17.3", format!("raw-window_test fill answered `{filled}`"))?;
    let verified = r.ask(&format!("verify {id} 77")).cite("17.3")?;
    let parts: Vec<&str> = verified.split(' ').collect();
    ensure(parts.first() == Some(&"ok") && parts.get(3) == Some(&"1"), "17.3", format!(
        "the raw client's buffer with no base after a commit: `{verified}`; it must equal the front, replayed in full"))?;
    // And through Breengel: the frame after the first holds the first.
    let mut bw = breengel::Window::new_without_fonts(b"full-copy", 48, 12).cite("17.3")?;
    {
        let fb = bw.framebuf();
        for y in 0..12u32 {
            for x in 0..48u32 {
                let p = pattern(55, x, y);
                fb.put_pixel(x as usize, y as usize, libgfx::color::Color::rgb(p[2], p[1], p[0]));
            }
        }
    }
    bw.present().cite("17.3")?;
    let info = bw.acquire(1_000_000_000).cite("17.3")?;
    ensure(info.replay_full, "17.3", "Breengel's second frame, a buffer with no base, was not replayed in full")?;
    let fb = bw.framebuf();
    for y in 0..12u32 {
        for x in 0..48u32 {
            let p = pattern(55, x, y);
            let got = fb.get_pixel(x as usize, y as usize);
            ensure((got.r, got.g, got.b) == (p[2], p[1], p[0]), "17.3", format!("Breengel's acquired frame has {:?} at ({x},{y}); the committed frame has {:02x?}", got, &p[..3]))?;
        }
    }
    Ok(())
}

fn windows_stale_base_commit() -> CaseResult {
    interface()?;
    let w = W::new(64, 16, 3)?;
    let id = w.id();
    // Front c1 = [1, 1].
    w.frame(1).cite("17.3")?;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE A {}", outcome(r))).cite("17.3")?;
    let b = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE B {}", outcome(r))).cite("17.3")?;
    // A changes cell 0 and commits (c2 = [2, 1]); B, still at c1, changes cell 1 and commits.
    fill_rect(w.buf(a.buffer), 64, cell(0), 2);
    w.commit(a.buffer, &[cell(0)]).map_err(|r| format!("COMMIT A {}", outcome(r))).cite("17.3")?;
    fill_rect(w.buf(b.buffer), 64, cell(1), 3);
    let cb = w.commit(b.buffer, &[cell(1)]).map_err(|r| format!("COMMIT of the stale buffer B {}", outcome(r))).cite("17.3")?;
    // The frame is B's whole content, [1, 3].
    let (_, px) = capture(id, 64, 16, None).map_err(|r| format!("CAPTURE {}", outcome(r))).cite("17.3")?;
    same(&px, 64, 16, false, "the frame after the stale commit", "17.3", cells_expect(1, 3))?;
    // Its damage reaches cell 0 (changed back from 2 to 1): A's buffer (base c2) replays it.
    let r = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE after the stale commit {}", outcome(r))).cite("17.3")?;
    same(w.buf(r.buffer), 64, 16, false, &format!("buffer {} replayed to the stale commit {}", r.buffer, cb.commit_id), "17.3", cells_expect(1, 3))?;
    let covers0 = r.replay_full == 1 || r.replay[..r.replay_count as usize].iter().any(|rc| rc.x <= 0 && rc.x + rc.w >= CELL_W as i32);
    ensure(r.base != 0 && covers0, "17.3", "the stale commit's damage, replayed, does not cover cell 0, which it changed")?;
    w.commit(r.buffer, &[]).map_err(|e| format!("COMMIT {}", outcome(e))).cite("17.3")?;

    // acquire-stall: thread 1's replay from front F pauses half way; thread 2 commits C and
    // then acquires; it gets F's buffer only after the copy ends.
    let w2 = std::sync::Arc::new(W::new(64, 64, 3)?);
    w2.frame(10).cite("17.3")?; // front F = seed 10
    let held = w2.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?; // thread 2's buffer
    // The third buffer has no base yet, so thread 1's ACQUIRE replays the whole window from F.
    libbreenix::test_control::arm(abi::tc_point::ACQUIRE_STALL, w2.id(), 0).map_err(|e| format!("test_control(ARM acquire-stall): {e}")).cite("21")?;
    let t1 = {
        let w2 = w2.clone();
        std::thread::spawn(move || w2.acquire(5_000_000_000))
    };
    let _ = time::sleep_ms(50);
    fill(w2.buf(held.buffer), 64, 64, 11);
    let cc = w2.commit(held.buffer, &[abi::Rect::new(0, 0, 64, 64)]).map_err(|r| format!("thread 2's COMMIT {}", outcome(r))).cite("17.3")?;
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let t2 = {
        let (w2, done) = (w2.clone(), done.clone());
        std::thread::spawn(move || {
            let r = w2.acquire(5_000_000_000);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            r
        })
    };
    let _ = time::sleep_ms(100);
    let early = done.load(std::sync::atomic::Ordering::SeqCst);
    let _ = libbreenix::test_control::arm(abi::tc_point::RELEASE_STALL, 0, 0);
    let r1 = t1.join().map_err(|_| String::from("thread 1 panicked")).cite("17.3")?.map_err(|r| format!("thread 1's stalled ACQUIRE {}", outcome(r))).cite("17.3")?;
    let r2 = t2.join().map_err(|_| String::from("thread 2 panicked")).cite("17.3")?.map_err(|r| format!("thread 2's ACQUIRE {}", outcome(r))).cite("17.3")?;
    let _ = libbreenix::test_control::disarm_all();
    let fired = libbreenix::test_control::fired(abi::tc_point::ACQUIRE_STALL).unwrap_or(0);
    ensure(fired >= 1, "21", "the acquire-stall control never fired")?;
    ensure(!early, "17.3", "thread 2's ACQUIRE returned while thread 1's replay from the old front was paused")?;
    let base_seed = if r1.base == cc.commit_id { 11 } else { 10 };
    same(w2.buf(r1.buffer), 64, 64, false, &format!("thread 1's buffer (base {})", r1.base), "17.3", |x, y| pattern(base_seed, x, y))?;
    same(w2.buf(r2.buffer), 64, 64, false, "thread 2's buffer", "17.3", |x, y| pattern(11, x, y))?;
    // More than eight commits after a held buffer's base: its commit gets whole-window damage.
    let w3 = W::new(64, 16, 3)?;
    w3.frame(1).cite("17.3")?;
    let held = w3.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    for s in 2..12 {
        w3.frame(s).cite("17.3")?;
    }
    let c = w3.commit(held.buffer, &[cell(0)]).map_err(|r| format!("COMMIT of a buffer ten commits stale {}", outcome(r))).cite("17.3")?;
    let next = w3.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    ensure(next.base == c.commit_id && next.replay_full == 1, "17.3", format!(
        "the next buffer's replay of a commit made ten commits after its buffer's base gave base {} with {} rectangles; the commit's damage is the whole window",
        next.base, next.replay_count))?;
    same(w3.buf(next.buffer), 64, 16, false, "the buffer after the stale commit", "17.3", |x, y| pattern(1, x, y))
}

// gpu-window: §2.2 accepted render work survives COMMIT; later front writes are refused without changing pixels.
fn windows_commit_immutable() -> CaseResult {
    interface()?;
    let w=W::new(16,16,2)?;
    let a=w.acquire(1_000_000_000).map_err(|r|format!("ACQUIRE {}",outcome(r))).cite("17.3")?;
    let mut context=bgl::Context::new().map_err(|r|format!("gpu-window: context {}",outcome(r))).cite("17.3")?;
    context.begin(w.c.buffer[a.buffer as usize],16,16);context.clear(0xff112233);
    let accepted=context.submit().map_err(|r|format!("gpu-window: acquired-target render {}",outcome(r))).cite("17.3")?;
    w.commit(a.buffer,&[full(&w.c)]).map_err(|r|format!("COMMIT {}",outcome(r))).cite("17.3")?;
    accepted.wait(2_000_000_000).map_err(|r|format!("gpu-window: pre-COMMIT fence {}",outcome(r))).cite("17.3")?;
    let (_,before)=capture(w.id(),16,16,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
    window_pixels(&before,16,16,"accepted render","17.3",|_,_|[51,34,17,255])?;
    context.begin(w.c.buffer[a.buffer as usize],16,16);context.clear(0xff556677);
    ensure(context.submit().is_err(),"17.3","gpu-window: submission wrote a committed target")?;
    let (_,after)=capture(w.id(),16,16,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
    ensure(after==before,"17.3","gpu-window: refused front write changed committed pixels")?;
    // A tree size below a page: acquiring the second buffer must not open the first
    // buffer's committed tree page.
    let ended = child_end(5_000, || {
        let mut c = abi::Create::ZERO;
        c.width = 16;
        c.height = 16;
        c.buffers = 2;
        c.tree_kib = 1;
        if call(op::CREATE, &mut c) < 0 { return 2; }
        if c.tree_kib % 4 != 0 { return 6; }
        let slot = |i: u32| (c.tree + i as u64 * c.tree_kib as u64 * 1024) as *mut u8;
        let Ok(a) = acquire(c.id, 1_000_000_000) else { return 3 };
        // SAFETY: CREATE mapped this buffer's tree page; it is writable while acquired.
        unsafe { *slot(a.buffer) = 1 };
        if commit(c.id, a.buffer, &[full(&c)], None).is_err() { return 4; }
        let Ok(b) = acquire(c.id, 1_000_000_000) else { return 3 };
        if b.buffer == a.buffer { return 3; }
        // SAFETY: as above; the committed tree page must fault.
        unsafe { core::ptr::write_volatile(slot(a.buffer), 2) };
        5
    }).cite("17.3")?;
    match ended {
        Ended::Signal(11) => Ok(()),
        Ended::Exit(5) => at("17.3", "with tree_kib 1, writing the committed buffer's tree page after acquiring the other buffer did not fault"),
        Ended::Exit(6) => at("17.1", "CREATE with tree_kib 1 reported a tree size that is not a whole number of 4 KiB pages"),
        Ended::Exit(c) => at("17.3", format!("the tree-page child exited {c} before the write")),
        Ended::Signal(s) => at("17.3", format!("the tree-page child died of signal {s}, not SIGSEGV")),
    }
}

fn windows_commit_busy() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let never = commit(w.id(), 1, &[], None).err().unwrap_or(0);
    want_errno(never, EBUSY, "17.3", "COMMIT of a buffer never acquired")?;
    let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    w.commit(a.buffer, &[]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
    want_errno(w.commit(a.buffer, &[]).err().unwrap_or(0), EBUSY, "17.3", "a second COMMIT of the same acquisition")
}

fn windows_thread_commit() -> CaseResult {
    interface()?;
    let w = std::sync::Arc::new(W::new(32, 32, 3)?);
    for round in 0..50 {
        let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
        let b = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let go = |buffer: u32| {
            let (w, barrier) = (w.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                w.commit(buffer, &[abi::Rect::new(0, 0, 32, 32)])
            })
        };
        let (t1, t2) = (go(a.buffer), go(b.buffer));
        let c1 = t1.join().map_err(|_| String::from("a thread panicked")).cite("17.3")?.map_err(|r| format!("round {round}: a thread's COMMIT {}", outcome(r))).cite("17.3")?;
        let c2 = t2.join().map_err(|_| String::from("a thread panicked")).cite("17.3")?.map_err(|r| format!("round {round}: a thread's COMMIT {}", outcome(r))).cite("17.3")?;
        let (lo, hi) = (c1.commit_id.min(c2.commit_id), c1.commit_id.max(c2.commit_id));
        ensure(hi == lo + 1, "17.3", format!("round {round}: concurrent commits got ids {} and {}; they are serialized, one apart", c1.commit_id, c2.commit_id))?;
    }
    Ok(())
}

fn windows_release_order() -> CaseResult {
    interface()?;
    // A two-buffer writer reuses each retired buffer as soon as it is free. If a buffer were freed
    // while a capture was still copying it, the capture would mix two frames; every capture must be
    // one whole commit. With kiosk_test composing every commit, composites pin the fronts too.
    let (_kiosk, _) = kiosk().cite("24")?;
    let w = std::sync::Arc::new(W::new(256, 128, 2)?);
    want_ok(w.register(b"release-order"), "17.1", "REGISTER")?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || -> Result<u64, String> {
            let mut n = 0;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let (c, px) = capture(w.id(), 256, 128, None).map_err(|r| format!("CAPTURE {}", outcome(r)))?;
                if c.commit_id == 0 { continue; }
                let seed = c.commit_id as u32;
                if let Some((x, y, got, want)) = first_diff(&px, 256, 128, false, |x, y| pattern(seed, x, y)) {
                    return Err(format!("a capture of commit {} has {:02x?} at ({x},{y}) where that commit has {:02x?}: a buffer was reused while it was being copied", c.commit_id, got, want));
                }
                n += 1;
            }
            Ok(n)
        })
    };
    for _ in 0..200 {
        let a = w.acquire(2_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
        // Commit ids rise by one per commit from 1, so commit k carries pattern k.
        let next = listed(w.id()).cite("17.3")?.map_or(0, |i| i.commit_id) + 1;
        fill(w.buf(a.buffer), 256, 128, next as u32);
        let c = w.commit(a.buffer, &[abi::Rect::new(0, 0, 256, 128)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
        ensure(c.commit_id == next, "17.3", format!("commit got id {} where {next} was next", c.commit_id))?;
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let captures = reader.join().map_err(|_| String::from("the reader panicked")).cite("17.3")?.cite("17.3")?;
    value("captures", captures as i64, "", None);
    ensure(captures > 0, "17.3", "the reader made no capture while 200 frames were committed")
}

fn windows_acquire_timeout() -> CaseResult {
    interface()?;
    let w = W::new(16, 16, 2)?;
    let _a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let _b = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    let start = now_ms();
    let r = w.acquire(200_000_000).err().unwrap_or(0);
    let took = since(start);
    value("timeout-wait", took as i64, "ms", Some((200, 300)));
    want_errno(r, ETIMEDOUT, "17.3", "ACQUIRE with both buffers held and a 200 ms timeout")?;
    ensure((200..=300).contains(&took), "17.5", format!("ACQUIRE timed out after {took} ms; a 200 ms timeout returns no earlier than 200 ms and no later than 300 ms"))
}

fn windows_resize() -> CaseResult {
    interface()?;
    let w = W::new(40, 30, 2)?;
    want_ok(w.register(b"resize"), "17.1", "REGISTER")?;
    w.frame(3).cite("17.3")?;
    let mut r = abi::Resize::ZERO;
    r.id = w.id();
    r.width = 60;
    r.height = 20;
    want_ok(call(op::RESIZE, &mut r), "17.1", "RESIZE 40x30 to 60x20")?;
    ensure(r.stride == 240, "17.1", format!("RESIZE gave stride {}; 60 pixels of BGRX are 240 bytes", r.stride))?;
    let info = listed(w.id()).cite("17.1")?.ok_or("the resized window is not listed (section 17.2)")?;
    ensure((info.width, info.height) == (60, 20), "17.1", format!("after RESIZE the window is listed {}x{}", info.width, info.height))?;
    let a = w.acquire(1_000_000_000).map_err(|e| format!("ACQUIRE after RESIZE {}", outcome(e))).cite("17.3")?;
    // SAFETY: RESIZE mapped each new buffer, 60x20x4 bytes, into the owner.
    let buf = unsafe { core::slice::from_raw_parts(r.buffer[a.buffer as usize] as *const u8, 60 * 20 * 4) };
    for y in 0..20u32 {
        for x in 0..40u32 {
            let at = ((y * 60 + x) * 4) as usize;
            let want = pattern(3, x, y);
            ensure(buf[at..at + 3] == want[..3], "17.1", format!("after RESIZE pixel ({x},{y}) is {:02x?}; the old contents are kept in the intersection, {:02x?}", &buf[at..at + 3], &want[..3]))?;
        }
    }
    resize_race()
}

/// Two threads of the owner resize one committed window, one to 64x64 and one to 1x1, 200
/// times each. Every resize succeeds, and the window ends at one of the two sizes with its
/// front's pixels in the intersection.
fn resize_race() -> CaseResult {
    let w = W::new(64, 64, 2)?;
    want_ok(w.register(b"resize-race"), "17.1", "REGISTER")?;
    w.frame(5).cite("17.3")?;
    let id = w.id();
    let resizer = move |size: u32| {
        std::thread::spawn(move || -> Result<(), String> {
            for i in 0..200 {
                let mut r = abi::Resize::ZERO;
                r.id = id;
                r.width = size;
                r.height = size;
                let ret = call(op::RESIZE, &mut r);
                if ret != 0 {
                    return Err(format!("resize {i} to {size}x{size} while another thread resizes {}", outcome(ret)));
                }
            }
            Ok(())
        })
    };
    let (big, small) = (resizer(64), resizer(1));
    for t in [big, small] {
        t.join().map_err(|_| String::from("a resizing thread panicked")).cite("17.2")?.cite("17.2")?;
    }
    let info = listed(id).cite("17.2")?.map(|i| (i.width, i.height)).ok_or("the window is not listed after concurrent resizes").cite("17.2")?;
    ensure(info == (64, 64) || info == (1, 1), "17.2", format!("after concurrent resizes the window is {}x{}; it is one of the two sizes asked for", info.0, info.1))?;
    let (_, px) = capture(id, info.0, info.1, None).map_err(|e| format!("CAPTURE after concurrent resizes {}", outcome(e))).cite("17.3")?;
    same(&px, 1, 1, false, "the top-left pixel after concurrent resizes", "17.2", |x, y| pattern(5, x, y))
}

fn windows_resize_error() -> CaseResult {
    interface()?;
    let w = W::new(20, 10, 2)?;
    let _ = read_events(w.id());
    for (rw, rh) in [(0u32, 10u32), (5000, 10)] {
        let mut r = abi::Resize::ZERO;
        r.id = w.id();
        r.width = rw;
        r.height = rh;
        want_errno(call(op::RESIZE, &mut r), EINVAL, "17.2", &format!("RESIZE to {rw}x{rh}"))?;
        let events = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e))).cite("19.2")?;
        ensure(!events.iter().any(|e| e.kind == abi::event_kind::RESIZED), "19.2", format!("a refused RESIZE to {rw}x{rh} delivered Resized"))?;
        let failed = events.iter().find(|e| e.kind == abi::event_kind::RESIZE_FAILED);
        ensure(failed.is_some_and(|e| (e.width, e.height) == (20, 10)), "19.2", format!(
            "a refused RESIZE to {rw}x{rh} delivered {:?}; ResizeFailed with the old 20x10 size is required", events.iter().map(|e| e.kind).collect::<Vec<_>>()))?;
    }
    Ok(())
}

// gpu-window: §2.2 charges GPU backing to RLIMIT_AS and the GPU quota; refusal is atomic.
fn windows_resize_rlimit() -> CaseResult {
    interface()?;
    child("the limited child", 10_000, None, || {
        let w = W::new(64, 64, 2).map_err(|e| format!("{e:?}"))?;
        w.frame(9)?;
        let _ = read_events(w.id());
        // VmSize is read just before and just after the RESIZE, so this child's own allocations
        // (libc keeps small blocks mapped for reuse) fall outside the comparison.
        let _ = vm_size()?;
        let base = vm_size()?;
        // Room for a little more, not for two 1024x1024 buffers.
        if set_rlimit_as(base + 1024 * 1024) != 0 {
            return Err(String::from("prlimit(RLIMIT_AS) failed"));
        }
        let gpu_before=crate::gpu_checks::counters()?;
        if gpu_before.resources<2 {return Err("gpu-window: window buffers are not GPU resources".into());}
        let mut r = abi::Resize::ZERO;
        r.id = w.id();
        r.width = 1024;
        r.height = 1024;
        let ret = call(op::RESIZE, &mut r);
        let after = vm_size()?;
        let gpu_after=crate::gpu_checks::counters()?;
        if gpu_after.resource_bytes!=gpu_before.resource_bytes || gpu_after.resources!=gpu_before.resources {return Err("gpu-window: refused resize changed GPU accounting".into());}
        if ret != -(ENOMEM as i64) {
            return Err(format!("RESIZE past RLIMIT_AS {}; the contract gives ENOMEM (section 17.2)", outcome(ret)));
        }
        if after != base {
            return Err(format!("the refused RESIZE changed VmSize from {base} to {after} bytes (section 17.2)"));
        }
        let (_, px) = capture(w.id(), 64, 64, None).map_err(|e| format!("CAPTURE after the refused RESIZE {}", outcome(e)))?;
        if let Some((x, y, _, _)) = first_diff(&px, 64, 64, false, |x, y| pattern(9, x, y)) {
            return Err(format!("after the refused RESIZE the window's pixel ({x},{y}) changed; the old buffers stay intact (section 17.2)"));
        }
        let a = w.acquire(1_000_000_000).map_err(|e| format!("ACQUIRE after the refused RESIZE {}", outcome(e)))?;
        if first_diff(w.buf(a.buffer), 64, 64, false, |x, y| pattern(9, x, y)).is_some() {
            return Err(String::from("after the refused RESIZE an acquired old buffer no longer holds the frame (section 17.2)"));
        }
        let events = read_events(w.id()).map_err(|e| format!("READ_EVENTS {}", outcome(e)))?;
        if !events.iter().any(|e| e.kind == abi::event_kind::RESIZE_FAILED && (e.width, e.height) == (64, 64)) {
            return Err(String::from("the refused RESIZE did not deliver ResizeFailed with the old size (section 19.2)"));
        }
        Ok(())
    }).cite("17.2")
}

fn windows_resize_rollback() -> CaseResult {
    interface()?;
    // CREATE of a 3-buffer 256x256 window allocates four pieces (three buffers, the tree pages);
    // RESIZE to 512x512 allocates three. RLIMIT_AS set to stop each allocation in turn must leave
    // the registry, the mappings (VmSize) and the next window id as they were. Each limit leaves
    // `slack` for this child's own allocations (libc maps each one), less than any piece.
    child("the limited child", 15_000, None, || {
        let piece = 256 * 256 * 4u64;
        let tree = 3 * 64 * 1024u64;
        let slack = 64 * 1024u64;
        // The slack must not fit any piece, or a step could succeed inside it.
        if slack >= tree.min(piece) {
            return Err(format!("the case's slack of {slack} bytes would fit a piece of {} bytes", tree.min(piece)));
        }
        let _ = vm_size()?;
        let base = vm_size()?;
        for step in 0..4u64 {
            let room = if step < 3 { step * piece } else { 3 * piece } + slack;
            if set_rlimit_as(base + room) != 0 { return Err(String::from("prlimit(RLIMIT_AS) failed")); }
            let mut c = abi::Create::ZERO;
            c.width = 256;
            c.height = 256;
            c.buffers = 3;
            let r = call(op::CREATE, &mut c);
            if r != -(ENOMEM as i64) {
                return Err(format!("CREATE stopped at allocation step {step} {}; ENOMEM is required (section 17.2)", outcome(r)));
            }
            let now = vm_size()?;
            if now != base {
                return Err(format!("CREATE refused at step {step} left VmSize {now} bytes where it was {base} (section 17.2)"));
            }
        }
        if set_rlimit_as(u64::MAX) != 0 { return Err(String::from("prlimit(RLIMIT_AS) failed")); }
        let w = W::new(256, 256, 3).map_err(|e| format!("{e:?}"))?;
        w.frame(4)?;
        // The capture buffer is allocated before any limit is set.
        let mut px = vec![0u8; 256 * 256 * 4];
        let _ = vm_size()?;
        let base = vm_size()?;
        let big = 512 * 512 * 4u64;
        for step in 0..3u64 {
            if set_rlimit_as(base + step * big + slack) != 0 { return Err(String::from("prlimit(RLIMIT_AS) failed")); }
            let mut r = abi::Resize::ZERO;
            r.id = w.id();
            r.width = 512;
            r.height = 512;
            let ret = call(op::RESIZE, &mut r);
            if ret != -(ENOMEM as i64) {
                return Err(format!("RESIZE stopped at allocation step {step} {}; ENOMEM is required (section 17.2)", outcome(ret)));
            }
            let now = vm_size()?;
            if now != base {
                return Err(format!("RESIZE refused at step {step} left VmSize {now} where it was {base} (section 17.2)"));
            }
            let mut c = abi::Capture::ZERO;
            c.id = w.id();
            c.dst = px.as_mut_ptr() as u64;
            c.dst_len = px.len() as u64;
            let got = call(op::CAPTURE, &mut c);
            if got < 0 {
                return Err(format!("CAPTURE after the refused RESIZE {}", outcome(got)));
            }
            if first_diff(&px, 256, 256, false, |x, y| pattern(4, x, y)).is_some() {
                return Err(format!("RESIZE refused at step {step} changed the window's pixels (section 17.2)"));
            }
        }
        Ok(())
    }).cite("17.2")
}

// gpu-window: §2.2 separates mapped trees, guest GPU backing under RLIMIT_AS, and the GPU quota.
fn windows_rlimit_accounting() -> CaseResult {
interface()?;
    let before=vm_size().cite("17.2")?;let gpu_before=crate::gpu_checks::counters().cite("17.2")?;
    let mut w=W::new(128,64,3)?;
    let mapped=vm_size().cite("17.2")?;let during=crate::gpu_checks::counters().cite("17.2")?;
    ensure(mapped-before==3*64*1024,"17.2","gpu-window: only tree pages belong in the mapped window charge")?;
    ensure(during.resources-gpu_before.resources==3 && during.resource_bytes-gpu_before.resource_bytes==3*128*64*4,
        "17.2","gpu-window: window GPU target charge is not exact")?;
    let over=192*1024*1024-during.resource_bytes+4096;
    let refused=libbreenix::gpu::buffer_create(over);
    if let Ok(h)=refused {libbreenix::gpu::resource_destroy(h);}
    ensure(refused==Err(-(ENOMEM as i64)),"17.2","gpu-window: window backing did not consume the process GPU quota")?;
    want_ok(w.destroy(),"17.2","DESTROY")?;
    ensure(vm_size().cite("17.2")?==before,"17.2","gpu-window: destroyed tree mappings remain charged")?;
    let start=now_ms();loop {let after=crate::gpu_checks::counters().cite("17.2")?;
        if after.resource_bytes==gpu_before.resource_bytes && after.resources==gpu_before.resources {break;}
        ensure(since(start)<1_000,"17.2","gpu-window: target charges did not return exactly to baseline")?;
        let _=time::sleep_ms(5);
    }Ok(())
}

fn windows_capture_across_destroy() -> CaseResult {
    interface()?;
    // One reader thread for every round, so a thread's own memory stays out of the count.
    let (to_reader, jobs) = std::sync::mpsc::channel::<u64>();
    let (results, from_reader) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut tree = vec![0u8; 64 * 1024];
        while let Ok(id) = jobs.recv() {
            let got = capture(id, 512, 512, Some(&mut tree));
            if results.send((got, tree[..8].to_vec())).is_err() {
                break;
            }
        }
    });
    let round = |round: u32| -> CaseResult {
        let mut w = W::new(512, 512, 2)?;
        let seed = 40 + round;
        let a = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
        fill(w.buf(a.buffer), 512, 512, seed);
        w.tree(a.buffer)[..8].copy_from_slice(&(seed as u64).to_le_bytes());
        w.commit(a.buffer, &[abi::Rect::new(0, 0, 512, 512)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
        // Even rounds destroy the window while `capture-stall` holds the capture half way
        // through its copy; odd rounds destroy it before the capture begins.
        let in_flight = round % 2 == 0;
        if in_flight {
            let fired = |point| libbreenix::test_control::fired(point).map_err(|e| format!("FIRED: {e}"));
            let stalls = fired(abi::tc_point::CAPTURE_STALL).cite("21")?;
            libbreenix::test_control::arm(abi::tc_point::CAPTURE_STALL, w.id(), 0).map_err(|e| format!("ARM capture-stall: {e}")).cite("21")?;
            to_reader.send(w.id()).map_err(|_| String::from("the reader thread is gone")).cite("17.3")?;
            let start = now_ms();
            while fired(abi::tc_point::CAPTURE_STALL).cite("21")? == stalls && since(start) < 1_000 {
                let _ = time::sleep_ms(2);
            }
            let paused = fired(abi::tc_point::CAPTURE_STALL).cite("21")? > stalls;
            let destroyed = w.destroy();
            let _ = libbreenix::test_control::arm(abi::tc_point::RELEASE_STALL, 0, 0);
            ensure(paused, "21", format!("round {round}: the capture-stall control did not fire within 1 s"))?;
            want_ok(destroyed, "17.2", "DESTROY during a paused capture")?;
        } else {
            want_ok(w.destroy(), "17.2", "DESTROY before a capture")?;
            to_reader.send(w.id()).map_err(|_| String::from("the reader thread is gone")).cite("17.3")?;
        }
        let (got, tree) = from_reader.recv().map_err(|_| String::from("the reader thread is gone")).cite("17.3")?;
        match (in_flight, got) {
            (false, Err(r)) => want_errno(r, ENOENT, "17.3", "a capture that began after DESTROY"),
            (false, Ok(_)) => at("17.3", format!("round {round}: a capture that began after DESTROY returned pixels; it is ENOENT")),
            (true, Err(r)) => at("17.3", format!("round {round}: a capture paused half way when its window was destroyed {}; it returns the whole commit", outcome(r))),
            (true, Ok((c, px))) => {
                same(&px, 512, 512, false, &format!("round {round}: a capture across DESTROY"), "17.3", |x, y| pattern(seed, x, y))?;
                let t = u64::from_le_bytes(tree[..8].try_into().unwrap());
                ensure(t == seed as u64 && c.commit_id == 1, "17.3", format!("round {round}: the capture's tree page says {t} for commit {}", c.commit_id))
            }
        }
    };
    // Rounds first, so the page tables the rounds need exist before the baseline; the
    // measured rounds must then return to it.
    for r in 0..3 {
        round(r)?;
    }
    let _ = mem_free_kb();
    let free_before = mem_free_kb().cite("17.3")?;
    let mut outcome_of_rounds = Ok(());
    for r in 0..20 {
        outcome_of_rounds = round(r);
        if outcome_of_rounds.is_err() {
            break;
        }
    }
    let free_after = mem_free_kb().cite("17.3")?;
    let _ = libbreenix::test_control::disarm_all();
    drop(to_reader);
    let _ = reader.join();
    outcome_of_rounds?;
    value("mem-free-change", free_after as i64 - free_before as i64, "kb", None);
    ensure(free_after + 64 >= free_before, "17.3", format!(
        "after 20 windows of 2 MiB were destroyed under captures, {} kB fewer are free; the buffers are freed when the copy ends", free_before - free_after))?;
    capture_across_resize()
}

/// A capture with a tree destination, paused half way by `capture-stall`, while its window is
/// resized and the owner commits the other buffer: the captured index's buffer is not handed
/// out until the copy ends, and the capture returns commit 1's pixels and tree page.
fn capture_across_resize() -> CaseResult {
    let fired = |point| libbreenix::test_control::fired(point).map_err(|e| format!("FIRED: {e}"));
    let w = W::new(64, 64, 2)?;
    let id = w.id();
    let first = w.acquire(1_000_000_000).map_err(|r| format!("ACQUIRE {}", outcome(r))).cite("17.3")?;
    fill(w.buf(first.buffer), 64, 64, 60);
    w.tree(first.buffer)[..8].copy_from_slice(&60u64.to_le_bytes());
    let c1 = w.commit(first.buffer, &[full(&w.c)]).map_err(|r| format!("COMMIT {}", outcome(r))).cite("17.3")?;
    let stalls = fired(abi::tc_point::CAPTURE_STALL).cite("21")?;
    libbreenix::test_control::arm(abi::tc_point::CAPTURE_STALL, id, 0).map_err(|e| format!("ARM capture-stall: {e}")).cite("21")?;
    let reader = std::thread::spawn(move || {
        let mut tree = vec![0u8; 64 * 1024];
        let got = capture(id, 64, 64, Some(&mut tree));
        (got, tree[..8].to_vec())
    });
    let start = now_ms();
    while fired(abi::tc_point::CAPTURE_STALL).cite("21")? == stalls && since(start) < 1_000 {
        let _ = time::sleep_ms(2);
    }
    let paused = fired(abi::tc_point::CAPTURE_STALL).cite("21")? > stalls;
    let mut r = abi::Resize::ZERO;
    r.id = id;
    r.width = 32;
    r.height = 32;
    let resized = call(op::RESIZE, &mut r);
    // The front keeps its index; the other buffer takes the next commit.
    let second = acquire(id, 1_000_000_000);
    let committed = second.map(|a| {
        w.tree(a.buffer)[..8].copy_from_slice(&61u64.to_le_bytes());
        commit(id, a.buffer, &[abi::Rect::new(0, 0, 32, 32)], None)
    });
    // The captured index's buffer is free now but for the capture: handing it out would make
    // its tree page writable, and the owner would write it.
    let early = acquire(id, 0);
    if let Ok(a) = early {
        w.tree(a.buffer)[..8].copy_from_slice(&0xEEu64.to_le_bytes());
    }
    let _ = libbreenix::test_control::arm(abi::tc_point::RELEASE_STALL, 0, 0);
    let (got, tree) = reader.join().map_err(|_| String::from("the reader thread panicked")).cite("17.3")?;
    let _ = libbreenix::test_control::disarm_all();
    ensure(paused, "21", "the capture-stall control did not fire within 1 s")?;
    want_ok(resized, "17.2", "RESIZE during a paused capture")?;
    second.map_err(|r| format!("ACQUIRE after the resize {}", outcome(r))).cite("17.3")?;
    committed.map_err(|r| format!("ACQUIRE after the resize {}", outcome(r))).cite("17.3")?
        .map_err(|r| format!("COMMIT after the resize {}", outcome(r))).cite("17.3")?;
    ensure(early.err() == Some(-(abi::errno::ETIMEDOUT as i64)), "17.3", format!(
        "ACQUIRE handed out buffer {:?}, whose tree page a paused capture was copying; it waits for the copy", early.map(|a| a.buffer)))?;
    let (c, px) = got.map_err(|r| format!("the capture across the resize {}", outcome(r))).cite("17.3")?;
    same(&px, 64, 64, false, "a capture across a resize", "17.3", |x, y| pattern(60, x, y))?;
    let t = u64::from_le_bytes(tree[..8].try_into().unwrap());
    ensure(c.commit_id == c1.commit_id && t == 60, "17.3", format!(
        "the capture across a resize returned commit {} with tree page {t}; commit {} wrote 60", c.commit_id, c1.commit_id))
}

fn windows_two_buffer_capture_load() -> CaseResult {
    interface()?;
    let w = std::sync::Arc::new(W::new(512, 256, 2)?);
    want_ok(w.register(b"capture-load"), "17.1", "REGISTER")?;
    let id = w.id();
    // An evidence holder: a root child that registers and stays alive.
    let mut holder_body = || {
        let r = call_id(op::HOLD_EVIDENCE, id);
        let _ = io::write(libbreenix::types::Fd::STDOUT, format!("{r}\n").as_bytes());
        let mut b = [0u8; 1];
        let _ = io::read(libbreenix::types::Fd::STDIN, &mut b);
        0
    };
    let mut holder = Helper::call(&mut holder_body).cite("17.3")?;
    let held = holder.line(5_000).cite("17.3")?;
    ensure(held == "0", "17.3", format!("HOLD_EVIDENCE by a root process answered {held}"))?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        std::thread::spawn(move || -> Result<(u64, u64), String> {
            let (mut n, mut longest) = (0, 0);
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let t = now_ms();
                capture(id, 512, 256, None).map_err(|r| format!("CAPTURE {}", outcome(r)))?;
                longest = longest.max(since(t));
                n += 1;
            }
            Ok((n, longest))
        })
    };
    let mut longest_wait = 0;
    for k in 0..300u32 {
        let t = now_ms();
        let a = w.acquire(2_000_000_000).map_err(|r| format!("ACQUIRE {k} {}", outcome(r))).cite("17.3")?;
        longest_wait = longest_wait.max(since(t));
        fill_rect(w.buf(a.buffer), 512, abi::Rect::new(0, (k % 256) as i32, 512, 1), k);
        w.commit(a.buffer, &[abi::Rect::new(0, (k % 256) as i32, 512, 1)]).map_err(|r| format!("COMMIT {k} {}", outcome(r))).cite("17.3")?;
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let (captures, longest_capture) = reader.join().map_err(|_| String::from("the reader panicked")).cite("17.3")?.cite("17.3")?;
    value("acquire-wait-max", longest_wait as i64, "ms", None);
    value("capture-max", longest_capture as i64, "ms", None);
    ensure(captures > 0, "17.3", "the reader made no capture")?;
    // One capture copy (no compositor runs here) plus one scheduler time slice to be woken.
    ensure(longest_wait <= longest_capture + 10, "17.3", format!(
        "the longest ACQUIRE waited {longest_wait} ms; with captures of at most {longest_capture} ms it may wait one copy (plus 10 ms to be scheduled)"))
}

fn windows_remnant_on_death() -> CaseResult {
    interface()?;
    // The owner: a child that draws, writes its tree page, commits and waits to be killed.
    let mut owner_body = || {
        let Ok(w) = W::new(64, 32, 2) else { return 2 };
        if w.register(b"remnant").is_negative() { return 3; }
        let Ok(a) = w.acquire(1_000_000_000) else { return 4 };
        fill(w.buf(a.buffer), 64, 32, 21);
        w.tree(a.buffer)[..16].copy_from_slice(b"remnant-tree-ok!");
        if w.commit(a.buffer, &[abi::Rect::new(0, 0, 64, 32)]).is_err() { return 5; }
        let _ = io::write(libbreenix::types::Fd::STDOUT, format!("{}\n", w.id()).as_bytes());
        core::mem::forget(w);
        let mut b = [0u8; 1];
        let _ = io::read(libbreenix::types::Fd::STDIN, &mut b);
        0
    };
    let mut owner = Helper::call(&mut owner_body).cite("17.3")?;
    let id = ids_from(&mut owner, 1).cite("17.3")?[0];
    // This case's process is root and neither owner nor compositor: it is the holder.
    let free_before = mem_free_kb().cite("17.3")?;
    want_ok(call_id(op::HOLD_EVIDENCE, id), "17.3", "HOLD_EVIDENCE by root")?;
    owner.kill_checked().cite("24")?;
    let mut c = abi::Capture::ZERO;
    let mut px = vec![0u8; 64 * 32 * 4];
    let mut tree = vec![0u8; 64 * 1024];
    c.id = id;
    c.dst = px.as_mut_ptr() as u64;
    c.dst_len = px.len() as u64;
    c.tree_dst = tree.as_mut_ptr() as u64;
    c.tree_len = tree.len() as u64;
    want_ok(call(op::READ_REMNANT, &mut c), "17.3", "READ_REMNANT after the owner was killed")?;
    same(&px, 64, 32, false, "the remnant", "17.3", |x, y| pattern(21, x, y))?;
    ensure(&tree[..16] == b"remnant-tree-ok!", "17.3", "the remnant's tree page is not the dead owner's last tree")?;
    want_ok(call_id(op::RELEASE_REMNANT, id), "17.3", "RELEASE_REMNANT")?;
    want_errno(call(op::READ_REMNANT, &mut c), ENOENT, "17.3", "READ_REMNANT after RELEASE_REMNANT")?;
    let free_after = mem_free_kb().cite("17.3")?;
    value("mem-free-change", free_after as i64 - free_before as i64, "kb", None);
    ensure(free_after + 16 >= free_before, "17.3", format!("after RELEASE_REMNANT {} kB fewer are free than before the owner died", free_before.saturating_sub(free_after)))
}

fn windows_churn() -> CaseResult {
    interface()?;
    // Cycles first, and every read below once, so page tables the mappings need, and the
    // blocks the reads allocate (libc keeps small ones mapped), exist before the baseline.
    let cycle = || -> CaseResult {
        let mut w = W::new(64, 64, 2)?;
        want_ok(w.register(b"churn"), "17.1", "REGISTER")?;
        want_ok(w.destroy(), "17.2", "DESTROY").map(|_| ())
    };
    for _ in 0..10 {
        cycle()?;
    }
    let _ = (list(), counter("window_buffer_allocs"), vm_size(), mem_free_kb());
    let listed_before = list().cite("17.2")?.len();
    let allocs_before = counter("window_buffer_allocs").cite("18")?;
    let vm_before = vm_size().cite("17.2")?;
    let free_before = mem_free_kb().cite("17.2")?;
    for _ in 0..100 {
        cycle()?;
    }
    let free_after = mem_free_kb().cite("17.2")?;
    let allocs = counter("window_buffer_allocs").cite("18")? - allocs_before;
    value("buffer-allocs", allocs as i64, "", Some((200, 200)));
    ensure(allocs == 200, "18", format!("100 two-buffer windows counted {allocs} buffer allocations; each counts 2"))?;
    ensure(list().cite("17.2")?.len() == listed_before, "17.2", "after 100 create and destroy cycles the registry lists a different number of windows")?;
    ensure(vm_size().cite("17.2")? == vm_before, "17.2", "after 100 create and destroy cycles VmSize differs from its baseline")?;
    value("mem-free-change", free_after as i64 - free_before as i64, "kb", None);
    ensure(free_after + 16 >= free_before, "17.2", format!("after 100 cycles {} kB fewer are free", free_before.saturating_sub(free_after)))
}

fn windows_distinct_content() -> CaseResult {
    let (width,_)=super::composition::screen_size()?;
    interface()?;
    let (mut kiosk, _) = kiosk().cite("24")?;
    let _ = kiosk.ask("auto 0");
    let wins: Vec<W> = (0..16).map(|_| W::new(80, 50, 2)).collect::<Result<_, _>>()?;
    for (i, w) in wins.iter().enumerate() {
        want_ok(w.register(b"distinct"), "17.1", "REGISTER")?;
        w.frame(w.id() as u32).cite("17.3")?;
        let (x, y) = (100 * (i as i32 % 4) + 10, 60 * (i as i32 / 4) + 10);
        let r = kiosk.ask(&format!("place {} {x} {y} {i} 1", w.id())).cite("24")?;
        ensure(r == "ok", "18", format!("kiosk_test place answered `{r}`"))?;
    }
    let r = kiosk.ask("composite").cite("24")?;
    ensure(r.starts_with("ok "), "18", format!("COMPOSITE answered `{r}`"))?;
    let screen = readback().cite("20.1")?;
    for (i, w) in wins.iter().enumerate() {
        let (ox, oy) = (100 * (i as u32 % 4) + 10, 60 * (i as u32 / 4) + 10);
        for y in 0..50 {
            for x in 0..80 {
                let at = ((oy + y) as usize * width + (ox + x) as usize) * 4;
                let want = pattern(w.id() as u32, x, y);
                ensure(screen[at..at + 3] == want[..3], "18", format!(
                    "window {} (seed {}) at screen ({},{}) shows {:02x?}; its own pixel is {:02x?}", i, w.id(), ox + x, oy + y, &screen[at..at + 3], &want[..3]))?;
            }
        }
    }
    Ok(())
}

/// The composed screen at the reported physical mode, by SCANOUT_READBACK (as root).
pub(crate) fn readback() -> Result<Vec<u8>, String> {
    let (width,height)=super::composition::screen_size().map_err(|e|format!("{e:?}"))?;
    let mut px = vec![0u8; width * height * 4];
    let mut rec = abi::ObservationRecord::ZERO;
    let mut rb = abi::Readback::ZERO;
    rb.dst = px.as_mut_ptr() as u64;
    rb.dst_len = px.len() as u64;
    rb.record = &mut rec as *mut abi::ObservationRecord as u64;
    rb.record_len = core::mem::size_of::<abi::ObservationRecord>() as u32;
    let r = call(op::SCANOUT_READBACK, &mut rb);
    if r < 0 {
        return Err(format!("SCANOUT_READBACK {}", outcome(r)));
    }
    Ok(px)
}

fn windows_raw_client() -> CaseResult {
    interface()?;
    let mut r = raw_client().cite("24")?;
    let created = r.ask("create 32 16 2").cite("24")?;
    let id: u64 = created.strip_prefix("ok ").and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("raw-window_test create answered `{created}`")).cite("17.1")?;
    let reg = r.ask(&format!("register {id} raw-client")).cite("17.1")?;
    ensure(reg == "ok", "17.1", format!("raw-window_test register answered `{reg}`"))?;
    let filled = r.ask(&format!("fill {id} 9")).cite("17.3")?;
    ensure(filled.starts_with("ok 1 "), "17.3", format!("raw-window_test's first commit answered `{filled}`; commit ids start at 1"))?;
    let info = listed(id).cite("17.1")?.ok_or("the raw client's window is not listed (section 17.2)")?;
    ensure(info.owner as i32 == r.pid && &info.title[..info.title_len as usize] == b"raw-client", "17.1", format!("LIST gives owner {} title {:?}", info.owner, String::from_utf8_lossy(&info.title[..info.title_len.min(64) as usize])))?;
    let (_, px) = capture(id, 32, 16, None).map_err(|e| format!("CAPTURE {}", outcome(e))).cite("17.3")?;
    same(&px, 32, 16, false, "the raw client's committed frame", "17.3", |x, y| pattern(9, x, y))?;
    let gone = r.ask(&format!("destroy {id}")).cite("17.2")?;
    ensure(gone == "ok", "17.2", format!("raw-window_test destroy answered `{gone}`"))?;
    ensure(listed(id).cite("17.2")?.is_none(), "17.2", "the raw client's destroyed window is still listed")
}

// claim-lint:ok: docs/gui/breengel.md specifies this assertion; the GPU lanes supply its implementation.
// gpu-window: §2.2 and ruling 47 use GPU backend 4 or 5, never a legacy framebuffer backend.
fn windows_every_target() -> CaseResult {
interface()?;
    let w=W::new(16,16,2)?;want_ok(w.register(b"every-target"),"17.1","REGISTER")?;
    let info=listed(w.id()).cite("17.1")?.ok_or("window not listed")?;
    let (r,caps)=libbreenix::gpu::caps();
    ensure(r==0 && [4,5].contains(&caps.backend) && info.backend==caps.backend,"20.2",
        format!("gpu-window: WinInfo backend {} does not match admitted GPU backend {}",info.backend,caps.backend))?;
    let mut pixels=vec![0;16*16*4];let mut record=[0;libbreenix::gpu::abi::record::SIZE];
    want_ok(libbreenix::gpu::readback(w.c.buffer[0],[0,0,16,16],&mut pixels,64,&mut record),"20.2","gpu-window: backend target readback")?;
    let word=|o|u32::from_le_bytes(record[o..o+4].try_into().unwrap());
    ensure(word(0)==2 && word(16)==caps.backend && word(20)==4,"20.2","gpu-window: target readback is not this GPU backend's record v2")
}
// gpu-window: §2.2 render handoff must not publish a pending target, and must refuse a failed render fence.
fn windows_commit_waits_render()->CaseResult {
    interface()?;
    for error in [false,true] {
        let mut k=super::composition::compositor()?;
        let p=super::composition::Placed::new(20,20,16,16,0,7);
        let w=W::new(16,16,3)?;super::composition::show(&mut k,&w,&p)?;super::composition::compose(&mut k)?;
        let (_,old)=capture(w.id(),16,16,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
        let a=w.acquire(1_000_000_000).map_err(|r|format!("ACQUIRE {}",outcome(r))).cite("17.3")?;
        let mut c=bgl::Context::new().map_err(|r|format!("context {}",outcome(r))).cite("17.3")?;
        c.begin(w.c.buffer[a.buffer as usize],16,16);c.clear(0xff112233);
        let stream=c.encoded().to_vec();
        use libbreenix::gpu::{self,abi as ga};
        libbreenix::test_control::arm(ga::test_point::WITHHOLD_FENCE,0,if error{ga::test_point::WITHHOLD_ONE}else{ga::test_point::WITHHOLD_DEPTH}).cite("21")?;
        let mut fences=Vec::new();
        for _ in 0..if error{1}else{4} {let (r,s)=gpu::submit(c.handle(),&stream);want_ok(r,"17.3","gpu-window: pending render")?;fences.push(s.fence);}
        for &fence in &fences {want_errno(gpu::wait(c.handle(),fence,0).0,ETIMEDOUT,"17.3","gpu-window: render must still be pending")?;}
        if error {
            want_errno(gpu::wait(c.handle(),fences[0],3_000_000_000).0,abi::errno::EIO,"17.3","gpu-window: held render watchdog")?;
            let commit=w.commit(a.buffer,&[full(&w.c)]);
            ensure(commit.err()==Some(-(abi::errno::EIO as i64)),"17.3","gpu-window: COMMIT accepted a failed render fence")?;
        } else {
            w.commit(a.buffer,&[full(&w.c)]).map_err(|r|format!("gpu-window: pending COMMIT {}",outcome(r))).cite("17.3")?;
            let (ret,before)=gpu::modes();want_ok(ret,"17.3","gpu-window: binding before pending composition")?;
            let (sent,received)=std::sync::mpsc::channel();
            let composing=std::thread::spawn(move||{let reply=k.ask("composite");let _=sent.send(reply);k});
            let _=time::sleep_ms(50);
            let early=match received.try_recv(){Ok(r)=>Some(r),Err(std::sync::mpsc::TryRecvError::Empty)=>None,
                Err(e)=>return at("17.3",format!("gpu-window: composition worker: {e}"))};
            if let Some(reply)=&early {ensure(matches!(reply,Ok(s) if s=="err 11"),"17.3","gpu-window: composition published a pending render instead of waiting or EAGAIN")?;}
            want_errno(gpu::wait(c.handle(),fences[3],0).0,ETIMEDOUT,"17.3","gpu-window: render still held during composition attempt")?;
            let (ret,pending)=gpu::modes();want_ok(ret,"17.3","gpu-window: pending binding")?;
            ensure(pending.binding_known==1 && pending.scanout_resource==before.scanout_resource,"17.3","gpu-window: pending rendering changed the scanout binding")?;
            let release=gpu::target_create(0,1,1).map_err(|r|format!("release target {}",outcome(r))).cite("17.3")?;
            c.begin(release,1,1);c.clear(0xff000000);
            let (r,last)=gpu::submit(c.handle(),c.encoded());want_ok(r,"17.3","gpu-window: release held render fences")?;
            gpu::resource_destroy(release);
            want_ok(gpu::wait(c.handle(),last.fence,2_000_000_000).0,"17.3","gpu-window: completed render")?;
            k=composing.join().map_err(|_|"gpu-window: composition worker panicked".to_string()).cite("17.3")?;
            let reply=match early{Some(r)=>r,None=>received.recv().map_err(|e|e.to_string())?}.cite("17.3")?;
            if reply=="err 11" {super::composition::compose(&mut k)?;}
            else {ensure(reply.starts_with("ok "),"17.3",format!("gpu-window: completed composition: {reply}"))?;}
            let (screen,_)=super::composition::screen()?;
            let (width,height)=super::composition::screen_size()?;
            same(&screen,width as u32,height as u32,true,"completed GPU render on scanout","17.3",|x,y|if (20..36).contains(&x) && (20..36).contains(&y){[51,34,17,255]}else{[0,0,0,255]})?;
            let (_,front)=capture(w.id(),16,16,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
            window_pixels(&front,16,16,"completed render","17.3",|_,_|[51,34,17,255])?;
        }
        libbreenix::test_control::disarm_all().cite("21")?;
        if error {let (_,front)=capture(w.id(),16,16,None).map_err(|r|format!("CAPTURE {}",outcome(r))).cite("17.3")?;
            ensure(front==old,"17.3","gpu-window: failed render changed the previous front")?;}
    }Ok(())
}
