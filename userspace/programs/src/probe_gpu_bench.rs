//! C1 baseline through the production GPU API, after native CAPS admission.
use libbreenix::{gpu, time};
use gpu::abi::{context_state, fence_status, format, pipeline, stream::encode};

const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1600;
const DRAWS: u32 = 200;
const COPY_SIDE: u32 = 4096;
const COPY_BYTES: u64 = 64 << 20;

fn checked(result: i64, operation: &str) -> Result<(), String> {
    if result == 0 { Ok(()) } else { Err(format!("{operation}: {result}")) }
}

fn trial(context: u64, words: &[u32], kind: &str, trial: u32, work: u64, unit: &str) -> Result<(), String> {
    let begin = time::now_monotonic().map_err(|e| format!("monotonic begin: {e:?}"))?.as_nanos();
    let (result, submit) = gpu::submit(context, words);
    checked(result, "SUBMIT")?;
    if submit.fence == 0 { return Err(String::from("SUBMIT returned no fence")); }
    // Each trial is one validated submission. No per-draw wait or readback.
    let (result, wait) = gpu::wait(context, submit.fence, 30_000_000_000);
    checked(result, "WAIT")?;
    if wait.status != fence_status::OK {
        return Err(format!("WAIT status={} answer={:#x}", wait.status, wait.answer));
    }
    let end = time::now_monotonic().map_err(|e| format!("monotonic end: {e:?}"))?.as_nanos();
    let elapsed = end.checked_sub(begin).filter(|v| *v > 0).ok_or("monotonic interval did not advance")?;
    println!("GT710_BENCH {{\"kind\":\"{kind}\",\"trial\":{trial},\"begin_ns\":{begin},\"end_ns\":{end},\"elapsed_ns\":{elapsed},\"work\":{work},\"unit\":\"{unit}\",\"completion\":true}}");
    Ok(())
}

pub fn run() -> Result<(), String> {
    let context = gpu::context_create().map_err(|e| format!("CONTEXT_CREATE: {e}"))?;
    // Match the suite's normal warm-up boundary; exclude it from the baseline.
    let mut warmed = false;
    for _ in 0..6000 {
        let (result, counters) = gpu::counters(context);
        checked(result, "COUNTERS")?;
        match counters.state {
            context_state::LIVE => { warmed = true; break; }
            context_state::WARMING => time::sleep_ms(5).map_err(|e| format!("warm-up sleep: {e:?}"))?,
            state => return Err(format!("context left warm-up with state {state}")),
        }
    }
    if !warmed { return Err(String::from("catalogue warm-up timeout")); }
    let target = gpu::target_create(format::B8G8R8A8, WIDTH, HEIGHT).map_err(|e| format!("draw target: {e}"))?;
    let source = gpu::target_create(format::B8G8R8A8, COPY_SIDE, COPY_SIDE).map_err(|e| format!("copy source: {e}"))?;
    let destination = gpu::target_create(format::B8G8R8A8, COPY_SIDE, COPY_SIDE).map_err(|e| format!("copy destination: {e}"))?;

    let mut draws = Vec::new();
    encode::begin(&mut draws, target);
    for draw in 0..DRAWS {
        // SOLID is the existing shaded quad pipeline, not CLEAR. Changing the
        // colour distinguishes successive full-screen draws in the production encoder.
        encode::draw(&mut draws, pipeline::SOLID, 0xff000000 | draw,
            &[[0, 0, WIDTH as i32, HEIGHT as i32]]);
    }
    let mut copy = Vec::new();
    // Disjoint device targets: COPY uses the existing CE transfer path, with
    // no sampled draw, CPU upload, overlap scratch or readback in the interval.
    encode::copy(&mut copy, source, [0, 0, COPY_SIDE, COPY_SIDE], destination, [0, 0]);
    for n in 1..=3 {
        trial(context, &draws, "core", n, u64::from(DRAWS) * u64::from(WIDTH) * u64::from(HEIGHT), "fragments")?;
    }
    for n in 1..=3 {
        trial(context, &copy, "memory", n, COPY_BYTES, "bytes")?;
    }
    for resource in [target, source, destination] { checked(gpu::resource_destroy(resource), "RESOURCE_DESTROY")?; }
    checked(gpu::context_destroy(context), "CONTEXT_DESTROY")
}
