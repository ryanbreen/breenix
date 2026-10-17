//! `submit`: command submission and fences (`docs/gui/gpu.md` section 1.4.1; layouts and
//! codes in `docs/gui/gpu-abi.md`).

use libbreenix::gpu;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self, Itimerval, Sigaction, Timeval, ITIMER_REAL, SIGALRM, SIGKILL};
use libbreenix::suite::{value, wait_for, CaseResult};
use libbreenix::{io, time};

use super::common::*;

const RED: u32 = bgra(0, 0, 0xFF, 0xFF);
const GREEN: u32 = bgra(0, 0xFF, 0, 0xFF);
const BLUE: u32 = bgra(0xFF, 0, 0, 0xFF);
const GREY: u32 = bgra(0x40, 0x40, 0x40, 0xFF);

pub fn context_create() -> CaseResult {
    let _case = begin()?;
    let start = now_ns();
    let ctx = context()?;
    let ms = ms_since(start);
    value("create-ms", ms as i64, "ms", Some((0, 10_000)));
    ensure(ms <= 10_000, || format!("CONTEXT_CREATE took {ms} ms"))?;
    ensure(ctx != 0, || "CONTEXT_CREATE returned handle 0".to_string())?;
    let c = caps()?;
    // Creation warms the fixed rendering catalogue; the profile-only `probe_int` pair
    // waits for the context's first PROBE (gpu-abi.md, CONTEXT_CREATE).
    let probe = c.pipelines & 1 << pipeline::PROBE_INT != 0;
    let programs = 2 * (c.pipelines & !(1 << pipeline::PROBE_INT)).count_ones() as u64;
    let k = counters(ctx)?;
    ensure(k.warmed == programs, || format!("creation warmed {} programs; the fixed rendering catalogue built so far has {programs}", k.warmed))?;
    ensure(k.state == context_state::LIVE, || format!("the new context's state is {}", k.state))?;
    // Owned: it draws and its work completes.
    let t = target(format::B8G8R8A8, 4, 4)?;
    frame(ctx, &scene(t, GREEN, &[]))?;
    expect(t, &scene_canvas(4, 4, GREEN, &[]), "the new context's first frame")?;
    if probe {
        // The first PROBE compiles and warms `probe_int`: integer addition into one pixel.
        let mut s = scene(t, GREEN, &[]);
        s.extend_from_slice(&[stream::cmd::PROBE, 24, 0, 0, 1, 1, 2, 2, 0x1234_0000, 0x5678]);
        s.resize(s.len() + 14, 0);
        let start = now_ns();
        frame(ctx, &s)?;
        value("first-probe-ms", ms_since(start) as i64, "ms", None);
        expect(t, &scene_canvas(4, 4, GREEN, &[([1, 1, 2, 2], 0x1234_5678)]), "the context's first PROBE")?;
        let k = counters(ctx)?;
        ensure(k.warmed == programs + 2, || format!("after the first PROBE the context has {} programs, expected {}", k.warmed, programs + 2))?;
    }
    pass()
}

pub fn context_destroy() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    // Hold the submission's completion back so its work is known to be outstanding.
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ONE)?;
    let f = submit(ctx, &scene(t, RED, &[]))?;
    ensure(gpu::context_destroy(ctx) == 0, || "CONTEXT_DESTROY refused a live context".to_string())?;
    let (r, _) = gpu::wait(ctx, f, 0);
    ensure(r == -EINVAL, || format!("WAIT on a destroyed context's handle returned {}", errno(r)))?;
    let p = counters(0)?;
    ensure(p.contexts == 0, || format!("{} context handles remain", p.contexts))?;
    ensure(p.device_contexts == 1, || format!("{} device contexts while the destroyed context's work is outstanding, expected 1", p.device_contexts))?;
    // The watchdog ends the withheld work at 2 s; then the context goes.
    let start = now_ns();
    wait_for(3_000, "freed-ms");
    loop {
        let p = counters(0)?;
        if p.device_contexts == 0 {
            break;
        }
        if ms_since(start) > 3_000 {
            return Err(fail_msg(format!("{} device contexts 3 s after the work's deadline", p.device_contexts)));
        }
        let _ = time::sleep_ms(20);
    }
    value("freed-ms", ms_since(start) as i64, "ms", None);
    ensure(gpu::context_destroy(ctx) == -EINVAL, || "a second CONTEXT_DESTROY of the handle was accepted".to_string())
}

pub fn context_limit() -> CaseResult {
    let _case = begin()?;
    let mut ctxs = Vec::new();
    for i in 0..limits::CONTEXTS {
        ctxs.push(gpu::context_create().map_err(|e| fail_msg(format!("context {i} of the quota: {}", errno(e))))?);
    }
    let r = gpu::context_create();
    ensure(r == Err(-EAGAIN), || format!("a context past the quota of {}: {r:?}", limits::CONTEXTS))?;
    ensure(gpu::context_destroy(ctxs[0]) == 0, || "CONTEXT_DESTROY refused".to_string())?;
    let again = gpu::context_create();
    ensure(again.is_ok(), || format!("after one was destroyed, CONTEXT_CREATE returned {again:?}"))
}

pub fn stream_validate() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let mut cases: Vec<(&str, Vec<u32>, u32, u32)> = Vec::new();
    let mut s = scene(t, RED, &[]);
    s.extend([0x77, 2]);
    cases.push(("an unknown command", s, 2, stream::error::UNKNOWN_COMMAND));
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    s.extend([stream::cmd::CLEAR, 40, RED]);
    cases.push(("a command longer than the stream", s, 1, stream::error::TRUNCATED));
    let mut s = Vec::new();
    encode::clear(&mut s, RED);
    cases.push(("a clear before any target", s, 0, stream::error::NO_TARGET));
    let mut s = Vec::new();
    encode::begin(&mut s, t ^ 0x5555);
    cases.push(("a handle the process does not own", s, 0, stream::error::BAD_HANDLE));
    let mut s = scene(t, RED, &[]);
    encode::draw(&mut s, pipeline::SOLID, BLUE, &[[1, 1, 2, 2], [5, 5, 4, 6]]);
    cases.push(("an inverted rectangle", s, 2, stream::error::BAD_RECT));
    let mut s = scene(t, RED, &[]);
    encode::draw(&mut s, pipeline::COUNT + 3, BLUE, &[[1, 1, 2, 2]]);
    cases.push(("a pipeline outside the catalogue", s, 2, stream::error::BAD_PIPELINE));
    let mut s = scene(t, RED, &[]);
    s.extend([stream::cmd::DRAW, 9, pipeline::SOLID, BLUE, 2, 0, 0, 1, 1]);
    cases.push(("a draw whose length disagrees with its count", s, 2, stream::error::BAD_LENGTH));
    for (what, s, index, error) in &cases {
        let (r, a) = gpu::submit(ctx, s);
        ensure(r == -EINVAL, || format!("{what}: SUBMIT returned {}", errno(r)))?;
        ensure(a.error_index == *index && a.error == *error, || {
            format!("{what}: refused at command {} with code {}, expected command {index} code {error}", a.error_index, a.error)
        })?;
    }
    let s = scene(t, RED, &[]);
    let (r, a) = gpu::submit_raw(ctx, s.as_ptr() as u64, 10);
    ensure(r == -EINVAL && a.error == stream::error::STREAM_LENGTH, || format!("a 10-byte stream: {} code {}", errno(r), a.error))?;
    let (r, a) = gpu::submit_raw(ctx, s.as_ptr() as u64, 0);
    ensure(r == -EINVAL && a.error == stream::error::STREAM_LENGTH, || format!("an empty stream: {} code {}", errno(r), a.error))?;
    ensure(counters(ctx)?.submissions == 0, || "a refused stream was counted as a submission".to_string())
}

pub fn validation_atomic() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 32, 32)?;
    frame(ctx, &scene(t, GREEN, &[]))?;
    let (_, rec) = read(t, 32, 32)?;
    let generation = u64_at(&rec, record::GENERATION);
    let mut bad = scene(t, RED, &[([0, 0, 16, 16], BLUE)]);
    encode::draw(&mut bad, pipeline::SOLID, BLUE, &[[9, 9, 3, 3]]);
    let (r, a) = gpu::submit(ctx, &bad);
    ensure(r == -EINVAL && a.error_index == 3, || format!("the stream with a bad fourth command: {} at command {}", errno(r), a.error_index))?;
    ensure(counters(ctx)?.submissions == 1, || "the refused stream reached the device's count".to_string())?;
    let rec = expect(t, &scene_canvas(32, 32, GREEN, &[]), "after the refused stream")?;
    let g = u64_at(&rec, record::GENERATION);
    ensure(g == generation, || format!("the target's generation moved from {generation} to {g}"))?;
    let mut bound = Vec::new();
    encode::begin(&mut bound, t);
    frame(ctx, &bound)?;
    let rec = expect(t, &scene_canvas(32, 32, GREEN, &[]), "after binding without writing")?;
    ensure(u64_at(&rec, record::GENERATION) == generation, || "BEGIN advanced the target's generation".to_string())
}

pub fn stream_overflow() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 16, 16)?;
    frame(ctx, &scene(t, GREEN, &[]))?;
    // Draws: 65,536 are a submission; one more is refused at that command.
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    for _ in 0..limits::DRAWS {
        encode::draw(&mut s, pipeline::SOLID, RED, &[]);
    }
    frame(ctx, &s)?;
    encode::draw(&mut s, pipeline::SOLID, RED, &[[0, 0, 16, 16]]);
    let (r, a) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && a.error == stream::error::DRAW_LIMIT && a.error_index == limits::DRAWS as u32 + 1, || {
        format!("{} draws: {} code {} at command {}", limits::DRAWS + 1, errno(r), a.error, a.error_index)
    })?;
    // Instances: 1,048,576 rectangles are a submission; one more is refused.
    // `cover`: the first rectangle covers the target, so a truncated stream would show.
    let many = |n: usize, cover: bool| {
        let mut s = Vec::with_capacity(9 + 4 * n);
        encode::begin(&mut s, t);
        s.extend([stream::cmd::DRAW, 5 + 4 * n as u32, pipeline::SOLID, RED, n as u32]);
        if cover {
            s.extend([0, 0, 16, 16]);
        }
        s.resize(9 + 4 * n, 0);
        s
    };
    let s = many(limits::INSTANCES as usize, false);
    frame(ctx, &s)?;
    drop(s);
    let s = many(limits::INSTANCES as usize + 1, true);
    let (r, a) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && a.error == stream::error::INSTANCE_LIMIT && a.error_index == 1, || {
        format!("{} instances: {} code {} at command {}", limits::INSTANCES + 1, errno(r), a.error, a.error_index)
    })?;
    // Buffers: two buffers of 33 and 32 MiB pass the 64 MiB a submission may name.
    let b1 = gpu::buffer_create(33 << 20).map_err(|e| fail_msg(format!("a 33 MiB buffer: {}", errno(e))))?;
    let b2 = gpu::buffer_create(32 << 20).map_err(|e| fail_msg(format!("a 32 MiB buffer: {}", errno(e))))?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::draw_buffer(&mut s, pipeline::SOLID, RED, b1, 0, 1);
    frame(ctx, &s)?;
    encode::draw_buffer(&mut s, pipeline::SOLID, RED, b2, 0, 1);
    let (r, a) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && a.error == stream::error::BUFFER_LIMIT && a.error_index == 2, || {
        format!("65 MiB of buffers: {} code {} at command {}", errno(r), a.error, a.error_index)
    })?;
    // Refused whole: nothing of the refused streams drew, and they were not counted.
    ensure(counters(ctx)?.submissions == 4, || format!("{} submissions counted, expected the 4 accepted", counters(ctx).map_or(0, |c| c.submissions)))?;
    expect(t, &scene_canvas(16, 16, GREEN, &[]), "after the refused streams")?;
    pass()
}

pub fn fence_order() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 16, 16)?;
    let colours: Vec<u32> = (0..8u8).map(|i| bgra(i * 30, 0x80, 0xFF - i * 30, 0xFF)).collect();
    let mut fences = Vec::new();
    for &c in &colours {
        fences.push(submit(ctx, &scene(t, c, &[]))?);
    }
    ensure(fences.windows(2).all(|w| w[0] < w[1]), || format!("fences are not increasing: {fences:x?}"))?;
    wait_ok(ctx, *fences.last().unwrap_or(&0), 2_000)?;
    for (i, &f) in fences.iter().enumerate() {
        let (r, _) = gpu::wait(ctx, f, 0);
        ensure(r == 0, || format!("the last fence completed but fence {i} answers {}", errno(r)))?;
    }
    expect(t, &scene_canvas(16, 16, colours[7], &[]), "after eight frames")?;
    pass()
}

pub fn fence_wait() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 24, 12)?;
    let (_, rec) = read(t, 24, 12)?;
    let g0 = u64_at(&rec, record::GENERATION);
    let rects = [([2, 3, 20, 9], BLUE)];
    let f = submit(ctx, &scene(t, GREY, &rects))?;
    let (r, _) = gpu::wait(ctx, f, 2_000_000_000);
    ensure(r == 0, || format!("WAIT returned {}", errno(r)))?;
    let rec = expect(t, &scene_canvas(24, 12, GREY, &rects), "the frame WAIT completed")?;
    let g = u64_at(&rec, record::GENERATION);
    ensure(g == g0 + 1, || format!("the target's generation is {g}, expected {}", g0 + 1))?;
    ensure(u64_at(&rec, record::RENDER_FENCE) != 0, || "the record names no render fence".to_string())
}

pub fn wait_timeout() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ONE)?;
    let f = submit(ctx, &scene(t, RED, &[]))?;
    // `docs/gui/gpu.md` section 1.4.1, timing bounds: [T, T + K] for T = 50 ms.
    let start = now_ns();
    let (r, _) = gpu::wait(ctx, f, 50_000_000);
    let ms = ms_since(start);
    value("wait-ms", ms as i64, "ms", Some((50, 300)));
    ensure(r == -ETIMEDOUT, || format!("WAIT on the incomplete fence returned {}", errno(r)))?;
    ensure((50..=300).contains(&ms), || format!("the 50 ms WAIT returned after {ms} ms, outside [50, 300]"))
}

extern "C" fn on_alarm(_sig: i32) {}

pub fn wait_eintr() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    signal::sigaction(SIGALRM, Some(&Sigaction::new(on_alarm)), None).map_err(|e| fail_msg(format!("sigaction: {e}")))?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ONE)?;
    let f = submit(ctx, &scene(t, RED, &[]))?;
    // `docs/gui/gpu.md` section 1.4.1, timing bounds: from arming the timer, [s, s + K]
    // for s = 30 ms.
    let timer = Itimerval { it_interval: Timeval { tv_sec: 0, tv_usec: 0 }, it_value: Timeval { tv_sec: 0, tv_usec: 30_000 } };
    let start = now_ns();
    signal::setitimer(ITIMER_REAL, &timer, None).map_err(|e| fail_msg(format!("setitimer: {e}")))?;
    let (r, _) = gpu::wait(ctx, f, 1_000_000_000);
    let ms = ms_since(start);
    value("interrupted-ms", ms as i64, "ms", Some((30, 280)));
    ensure(r == -EINTR, || format!("WAIT with SIGALRM due at 30 ms returned {} after {ms} ms", errno(r)))?;
    ensure((30..=280).contains(&ms), || format!("WAIT's EINTR came {ms} ms after the timer was armed, outside [30, 280]"))
}

pub fn wait_killable() -> CaseResult {
    let _case = begin()?;
    let (rd, wr) = io::pipe().map_err(|e| fail_msg(format!("pipe: {e}")))?;
    match process::fork().map_err(|e| fail_msg(format!("fork: {e}")))? {
        ForkResult::Child => {
            let ok = (|| -> Result<u64, ()> {
                let ctx = gpu::context_create().map_err(|_| ())?;
                let t = gpu::target_create(format::B8G8R8A8, 8, 8).map_err(|_| ())?;
                if gpu::tc_arm(test_point::WITHHOLD_FENCE, 0, test_point::WITHHOLD_ONE) != 0 {
                    return Err(());
                }
                let (r, a) = gpu::submit(ctx, &scene(t, RED, &[]));
                if r != 0 {
                    return Err(());
                }
                let _ = io::write(wr, b"w");
                let _ = gpu::wait(ctx, a.fence, 10_000_000_000);
                Ok(ctx)
            })();
            if ok.is_err() {
                let _ = io::write(wr, b"x");
            }
            process::exit(3);
        }
        ForkResult::Parent(child) => {
            let _ = io::close(wr);
            let mut b = [0u8; 1];
            let n = io::read(rd, &mut b).unwrap_or(0);
            if n != 1 || b[0] != b'w' {
                let _ = signal::kill(child.raw() as i32, SIGKILL);
                let _ = process::waitpid(child.raw() as i32, core::ptr::null_mut(), 0);
                return Err(fail_msg("the child could not submit its withheld frame".to_string()));
            }
            // Give the child time to be blocked in WAIT, then kill it.
            let _ = time::sleep_ms(50);
            let start = now_ns();
            signal::kill(child.raw() as i32, SIGKILL).map_err(|e| fail_msg(format!("kill: {e}")))?;
            let mut status = 0i32;
            process::waitpid(child.raw() as i32, &mut status, 0).map_err(|e| fail_msg(format!("waitpid: {e}")))?;
            let ms = ms_since(start);
            value("exit-ms", ms as i64, "ms", Some((0, 250)));
            ensure(process::wifsignaled(status) && process::wtermsig(status) == SIGKILL, || format!("the child ended with status {status:#x}, not by SIGKILL"))?;
            ensure(ms <= 250, || format!("the child killed in WAIT took {ms} ms to exit"))
        }
    }
}

pub fn fence_foreign() -> CaseResult {
    let _case = begin()?;
    let a = context()?;
    let b = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let fb = submit(b, &scene(t, RED, &[]))?;
    let (r, _) = gpu::wait(a, fb, 1_000_000_000);
    ensure(r == -EINVAL, || format!("WAIT on context A with context B's fence returned {}", errno(r)))?;
    let (r, _) = gpu::wait(b, fb + 1000, 0);
    ensure(r == -EINVAL, || format!("WAIT on a fence context B never issued returned {}", errno(r)))?;
    wait_ok(b, fb, 2_000)
}

pub fn multi_context() -> CaseResult {
    let _case = begin()?;
    let a = context()?;
    let b = context()?;
    let ta = target(format::B8G8R8A8, 32, 24)?;
    let tb = target(format::R8G8B8A8, 24, 32)?;
    let ra = [([3, 4, 20, 11], BLUE), ([25, 0, 40, 30], GREEN)];
    let rb = [([0, 0, 5, 5], RED), ([10, 20, 24, 32], BLUE)];
    let mut fences = Vec::new();
    for round in 0..3u8 {
        let bg = bgra(round * 40, 0x20, 0x10, 0xFF);
        fences.push((a, submit(a, &scene(ta, bg, &ra))?));
        fences.push((b, submit(b, &scene(tb, bg ^ 0x00FF_FF00, &rb))?));
    }
    for (ctx, f) in fences {
        wait_ok(ctx, f, 2_000)?;
    }
    let last = bgra(80, 0x20, 0x10, 0xFF);
    expect(ta, &scene_canvas(32, 24, last, &ra), "context A's target")?;
    expect(tb, &scene_canvas(24, 32, last ^ 0x00FF_FF00, &rb), "context B's target")?;
    pass()
}

pub fn ring_backpressure() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 64, 64)?;
    let rects = [([3, 5, 40, 30], GREY)];
    let colours: Vec<u32> = (0..5u8).map(|i| bgra(i * 50, 0x30, 0xFF - i * 50, 0xFF)).collect();
    // Four in flight, proved by their fences rather than by host GPU speed
    // (`docs/gui/gpu.md` section 1.4.1): point 31's fence-depth mode keeps the answers to
    // the context's first four submissions unpublished until a fifth SUBMIT waits for room.
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_DEPTH)?;
    let mut fences = Vec::new();
    for &c in &colours[..4] {
        fences.push(submit(ctx, &scene(t, c, &rects))?);
    }
    for (i, &f) in fences.iter().enumerate() {
        let (r, _) = gpu::wait(ctx, f, 0);
        ensure(r == -ETIMEDOUT, || format!("with four in flight, submission {i}'s fence answered {}", errno(r)))?;
    }
    ensure(counters(ctx)?.ring_waits == 0, || "a ring wait was counted before the fifth submission".to_string())?;
    fences.push(submit(ctx, &scene(t, colours[4], &rects))?);
    let k = counters(ctx)?;
    ensure(k.ring_waits == 1, || format!("{} ring waits counted, expected the fifth submission's one", k.ring_waits))?;
    // All five complete OK, in order.
    ensure(fences.windows(2).all(|w| w[0] < w[1]), || format!("fences are not increasing: {fences:x?}"))?;
    wait_ok(ctx, fences[4], 2_000)?;
    for (i, &f) in fences.iter().enumerate() {
        let (r, _) = gpu::wait(ctx, f, 0);
        ensure(r == 0, || format!("the fifth fence completed but fence {i} answers {}", errno(r)))?;
    }
    let k = counters(ctx)?;
    ensure(k.submissions == 5 && k.draws == 10, || format!("{} submissions and {} draws counted, expected 5 and 10", k.submissions, k.draws))?;
    // None is lost: the target shows the fifth frame.
    expect(t, &scene_canvas(64, 64, colours[4], &rects), "the fifth frame")?;
    pass()
}

pub fn device_error_eio() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let peer = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let peer_target = target(format::B8G8R8A8, 8, 8)?;
    frame(peer, &scene(peer_target, GREEN, &[]))?;
    let before = fired(test_point::DEVICE_ERROR);
    let mut fences = Vec::new();
    for n in 1..=2 {
        arm(test_point::DEVICE_ERROR, 0)?;
        let f = submit(ctx, &scene(t, RED, &[]))?;
        fences.push(f);
        let (r, w) = gpu::wait(ctx, f, 2_000_000_000);
        ensure(r == -EIO && w.status == fence_status::DEVICE_ERROR, || format!("error {n}: WAIT returned {} status {}", errno(r), w.status))?;
        let k = counters(ctx)?;
        ensure(k.device_errors == n, || format!("{} device errors counted, expected {n}", k.device_errors))?;
        let state = if n == 1 { context_state::LIVE } else { context_state::QUARANTINED };
        ensure(k.state == state, || format!("after error {n}: state {}, expected {state}", k.state))?;
    }
    ensure(fired(test_point::DEVICE_ERROR) == before + 2, || "point 27 did not fire twice".to_string())?;
    let (r, _) = gpu::submit(ctx, &scene(t, BLUE, &[]));
    ensure(r == -EIO, || format!("SUBMIT on the quarantined context returned {}", errno(r)))?;
    for f in fences {
        let (r, w) = gpu::wait(ctx, f, 0);
        ensure(r == -EIO && w.status == fence_status::DEVICE_ERROR, || "a device error's recorded answer changed".to_string())?;
    }
    // A reset would invalidate this pre-existing context and target.
    ensure(counters(peer)?.state == context_state::LIVE, || "the peer context was reset or quarantined".to_string())?;
    expect(peer_target, &scene_canvas(8, 8, GREEN, &[]), "the peer's pre-error pixels")?;
    frame(peer, &scene(peer_target, BLUE, &[]))?;
    expect(peer_target, &scene_canvas(8, 8, BLUE, &[]), "the peer's post-error draw")?;
    pass()
}

pub fn recovery_after_error() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 20, 20)?;
    arm(test_point::DEVICE_ERROR, 0)?;
    let f = submit(ctx, &scene(t, RED, &[]))?;
    let (r, _) = gpu::wait(ctx, f, 2_000_000_000);
    ensure(r == -EIO, || format!("the forced error's WAIT returned {}", errno(r)))?;
    let rects = [([1, 1, 9, 19], GREEN), ([12, 2, 20, 6], BLUE)];
    frame(ctx, &scene(t, GREY, &rects))?;
    expect(t, &scene_canvas(20, 20, GREY, &rects), "the frame after the device error")?;
    pass()
}

pub fn frame_one_syscall() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 64, 64)?;
    let rects: Vec<([i32; 4], u32)> = (0..1000u32)
        .map(|i| {
            let x = (i * 11) % 60;
            let y = (i * 17) % 60;
            ([x as i32, y as i32, x as i32 + 4, y as i32 + 4], bgra(i as u8, (i >> 8) as u8 * 50, 0x77, 0xFF))
        })
        .collect();
    frame(ctx, &scene(t, GREY, &rects))?;
    let k = counters(ctx)?;
    ensure(k.submissions == 1, || format!("{} submissions for one SUBMIT", k.submissions))?;
    ensure(k.draws == 1001, || format!("{} draws counted, expected a clear and 1000 draws", k.draws))?;
    expect(t, &scene_canvas(64, 64, GREY, &rects), "the 1000-draw frame")?;
    pass()
}

pub fn freed_resource_refused() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let keep = target(format::B8G8R8A8, 8, 8)?;
    let buf = gpu::buffer_create(4096).map_err(|e| fail_msg(format!("a buffer: {}", errno(e))))?;
    frame(ctx, &scene(t, RED, &[]))?;
    ensure(gpu::resource_destroy(t) == 0 && gpu::resource_destroy(buf) == 0, || "RESOURCE_DESTROY refused".to_string())?;
    let (r, a) = gpu::submit(ctx, &scene(t, GREEN, &[]));
    ensure(r == -EINVAL && a.error_index == 0 && a.error == stream::error::BAD_HANDLE, || format!("a stream naming the destroyed target: {} at {} code {}", errno(r), a.error_index, a.error))?;
    let mut s = scene(keep, GREEN, &[]);
    encode::draw_buffer(&mut s, pipeline::SOLID, RED, buf, 0, 1);
    let (r, a) = gpu::submit(ctx, &s);
    ensure(r == -EINVAL && a.error_index == 2 && a.error == stream::error::BAD_HANDLE, || format!("a stream naming the destroyed buffer: {} at {} code {}", errno(r), a.error_index, a.error))?;
    ensure(gpu::resource_destroy(t) == -EINVAL, || "a second RESOURCE_DESTROY was accepted".to_string())
}

pub fn fence_echo_checked() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let ta = target(format::B8G8R8A8, 8, 8)?;
    let tb = target(format::B8G8R8A8, 8, 8)?;
    let before = fired(test_point::STALE_FENCE);
    arm(test_point::STALE_FENCE, 0)?;
    // A's answer is made to claim the next fence, B's: neither may be taken as done by it.
    let fa = submit(ctx, &scene(ta, RED, &[]))?;
    let fb = submit(ctx, &scene(tb, BLUE, &[]))?;
    let (r, w) = gpu::wait(ctx, fa, 2_000_000_000);
    ensure(fired(test_point::STALE_FENCE) == before + 1, || "the stale-fence point did not fire".to_string())?;
    ensure(r == -EIO && w.status == fence_status::FENCE_MISMATCH, || format!("WAIT on the fence answered for another submission returned {} status {}", errno(r), w.status))?;
    wait_ok(ctx, fb, 2_000)?;
    expect(tb, &scene_canvas(8, 8, BLUE, &[]), "the other submission's target")?;
    pass()
}

pub fn counters_exact() -> CaseResult {
    let _case = begin()?;
    let ctx = context()?;
    let t = target(format::B8G8R8A8, 32, 16)?;
    let mut s = Vec::new();
    encode::begin(&mut s, t);
    encode::clear(&mut s, GREY);
    // 16 pixels, then 4 and 4 after clipping to the 32x16 target.
    encode::draw(&mut s, pipeline::SOLID, RED, &[[0, 0, 4, 4], [-2, -2, 2, 2], [30, 14, 40, 20]]);
    frame(ctx, &s)?;
    read(t, 32, 16)?;
    read(t, 32, 16)?;
    let area = 32 * 16 + 16 + 4 + 4;
    for (scope, k) in [("the context", counters(ctx)?), ("the process", counters(0)?)] {
        let got = [k.submissions, k.draws, k.instances, k.fragment_area, k.work_estimate, k.uploads_image, k.uploads_fixture, k.readbacks, k.ring_waits, k.device_errors];
        let want = [1, 2, 4, area, area, 0, 0, 2, 0, 0];
        ensure(got == want, || format!("{scope}'s counters are {got:?}, expected {want:?} (submissions, draws, instances, area, estimate, uploads, fixture uploads, readbacks, ring waits, device errors)"))?;
    }
    let p = counters(0)?;
    ensure(p.contexts == 1 && p.resources == 1 && p.resource_bytes == 4096, || {
        format!("the process holds {} contexts, {} resources and {} bytes, expected 1, 1 and 4096", p.contexts, p.resources, p.resource_bytes)
    })
}

pub fn watchdog_quarantine() -> CaseResult {
    let _case = begin()?;
    let hung = context()?;
    let other = context()?;
    let t = target(format::B8G8R8A8, 8, 8)?;
    let u = target(format::B8G8R8A8, 8, 8)?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ONE)?;
    // `docs/gui/gpu.md` section 1.4.1, timing bounds: [W, W + 2K], the lower bound from
    // the stalled SUBMIT's start and the upper from its return.
    let start = now_ns();
    let f = submit(hung, &scene(t, RED, &[]))?;
    let sent = now_ns();
    wait_for(2_000, "quarantined-ms");
    let (r, w) = gpu::wait(hung, f, 3_000_000_000);
    let (ms, after_sent) = (ms_since(start), ms_since(sent));
    value("quarantined-ms", ms as i64, "ms", Some((2_000, 2_500)));
    ensure(r == -EIO && w.status == fence_status::DEVICE_HUNG, || format!("WAIT on the withheld fence returned {} status {}", errno(r), w.status))?;
    ensure(ms >= 2_000 && after_sent <= 2_500, || {
        format!("the watchdog ended the WAIT {ms} ms after SUBMIT began and {after_sent} ms after it returned, outside [2000, 2500]")
    })?;
    ensure(counters(hung)?.state == context_state::QUARANTINED, || "the hung context is not quarantined".to_string())?;
    let (r, _) = gpu::submit(hung, &scene(t, GREEN, &[]));
    ensure(r == -EIO, || format!("a submission to the quarantined context returned {}", errno(r)))?;
    frame(other, &scene(u, BLUE, &[]))?;
    expect(u, &scene_canvas(8, 8, BLUE, &[]), "the other context's frame")?;
    pass()
}

pub fn device_reset_recovers() -> CaseResult {
    let _case = begin()?;
    let a = context()?;
    let b = context()?;
    let c = context()?;
    let ta = target(format::B8G8R8A8, 8, 8)?;
    let tb = target(format::B8G8R8A8, 8, 8)?;
    arm(test_point::WITHHOLD_FENCE, test_point::WITHHOLD_ALL)?;
    // `docs/gui/gpu.md` section 1.4.1, timing bounds: [2W, 2W + 3K + R], the lower bound
    // from the stalled SUBMIT's start and the upper from its return.
    let start = now_ns();
    let fa = submit(a, &scene(ta, RED, &[]))?;
    let sent = now_ns();
    wait_for(4_000, "reset-ms");
    let (r, w) = gpu::wait(a, fa, 3_000_000_000);
    ensure(r == -EIO && w.status == fence_status::DEVICE_HUNG, || format!("the stalled submission's WAIT returned {} status {}", errno(r), w.status))?;
    // The device now makes no progress: B's work stays outstanding until the reset.
    let fb = submit(b, &scene(tb, BLUE, &[]))?;
    let (r, w) = gpu::wait(b, fb, 4_000_000_000);
    let (ms, after_sent) = (ms_since(start), ms_since(sent));
    value("reset-ms", ms as i64, "ms", Some((4_000, 4_850)));
    ensure(r == -EIO && w.status == fence_status::DEVICE_RESET, || format!("WAIT across the reset returned {} status {}", errno(r), w.status))?;
    ensure(ms >= 4_000 && after_sent <= 4_850, || {
        format!("the reset ended the WAIT {ms} ms after the stalled SUBMIT began and {after_sent} ms after it returned, outside [4000, 4850]")
    })?;
    for (name, ctx) in [("A", a), ("B", b), ("C", c)] {
        let k = counters(ctx)?;
        ensure(k.state == context_state::RESET, || format!("context {name}'s state after the reset is {}", k.state))?;
        let (r, _) = gpu::submit(ctx, &scene(ta, GREEN, &[]));
        ensure(r == -EIO, || format!("a submission to context {name} after the reset returned {}", errno(r)))?;
    }
    let d = context()?;
    let td = target(format::B8G8R8A8, 16, 16)?;
    let rects = [([2, 2, 14, 7], GREEN)];
    frame(d, &scene(td, GREY, &rects))?;
    expect(td, &scene_canvas(16, 16, GREY, &rects), "new work after the reset")?;
    pass()
}
