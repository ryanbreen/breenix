//! C1 baseline through the production GPU API, after native CAPS admission.
use libbreenix::{gpu, time};
use gpu::abi::{context_state, fence_status, format, pipeline, stream::encode};

const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1600;
const DRAWS: u32 = 200;

fn checked(result: i64, operation: &str) -> Result<(), String> {
    if result == 0 { Ok(()) } else { Err(format!("{operation}: {result}")) }
}

fn trial(context: u64, words: &[u32], kind: &str, trial: u32, work: u64, unit: &str) -> Result<i128, String> {
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
    Ok(elapsed)
}

#[derive(Clone, Copy)]
pub enum ClockPhase { Post, Low, Restore }

/// C2 reuses C1's 200 shaded draws and completion boundary at each frequency.
pub fn run_c2(mut observe:impl FnMut(ClockPhase)->Result<(),String>)->Result<(),String> {
    let context=gpu::context_create().map_err(|e|format!("CONTEXT_CREATE: {e}"))?;
    let mut warmed=false;
    for _ in 0..6000 {
        let (result,counters)=gpu::counters(context);checked(result,"COUNTERS")?;
        match counters.state {
            context_state::LIVE=>{warmed=true;break;}
            context_state::WARMING=>time::sleep_ms(5).map_err(|e|format!("warm-up sleep: {e:?}"))?,
            state=>return Err(format!("context left warm-up with state {state}")),
        }
    }
    if !warmed {return Err(String::from("catalogue warm-up timeout"));}
    let target=gpu::target_create(format::B8G8R8A8,WIDTH,HEIGHT).map_err(|e|format!("draw target: {e}"))?;
    let mut draws=Vec::new();encode::begin(&mut draws,target);
    for draw in 0..DRAWS {encode::draw(&mut draws,pipeline::SOLID,0xff000000|draw,&[[0,0,WIDTH as i32,HEIGHT as i32]]);}
    for phase in 0..3 {
        let frequency=if phase==1 {540_000}else{810_000};
        observe(match phase {0=>ClockPhase::Post,1=>ClockPhase::Low,_=>ClockPhase::Restore})?;
        println!("GT710_C2_PHASE phase={phase} gpc_khz={frequency}");
        let trials=if phase==0 {9}else{3};
        let first=match phase {0=>0,1=>9,_=>12};
        for n in 1..=trials {
            trial(context,&draws,"core",first+n,u64::from(DRAWS)*u64::from(WIDTH)*u64::from(HEIGHT),"fragments")?;
        }
    }
    checked(gpu::resource_destroy(target),"RESOURCE_DESTROY")?;
    checked(gpu::context_destroy(context),"CONTEXT_DESTROY")
}

/// C3a keeps every clock at POST. Sensor errors are records, not a benchmark
/// failure; the observer aborts only for the temperature safety threshold.
pub fn run_c3a(mut observe:impl FnMut(&str,u32)->Result<(),String>)->Result<(),String> {
    observe("post",0)?;
    let context=gpu::context_create().map_err(|e|format!("CONTEXT_CREATE: {e}"))?;
    let mut warmed=false;
    for _ in 0..6000 {
        let (result,counters)=gpu::counters(context);checked(result,"COUNTERS")?;
        match counters.state {
            context_state::LIVE=>{warmed=true;break;}
            context_state::WARMING=>time::sleep_ms(5).map_err(|e|format!("warm-up sleep: {e:?}"))?,
            state=>return Err(format!("context left warm-up with state {state}")),
        }
    }
    if !warmed {return Err(String::from("catalogue warm-up timeout"));}
    let target=gpu::target_create(format::B8G8R8A8,WIDTH,HEIGHT).map_err(|e|format!("draw target: {e}"))?;
    let mut draws=Vec::new();encode::begin(&mut draws,target);
    for draw in 0..DRAWS {encode::draw(&mut draws,pipeline::SOLID,0xff000000|draw,&[[0,0,WIDTH as i32,HEIGHT as i32]]);}
    for n in 1..=12 {
        observe("bench",n)?;
        trial(context,&draws,"core",n,u64::from(DRAWS)*u64::from(WIDTH)*u64::from(HEIGHT),"fragments")?;
    }
    for second in 1..=10 {
        time::sleep_ms(1000).map_err(|e|format!("idle sleep: {e:?}"))?;
        observe("idle",second)?;
    }
    checked(gpu::resource_destroy(target),"RESOURCE_DESTROY")?;
    checked(gpu::context_destroy(context),"CONTEXT_DESTROY")
}

/// Proc data is decimal and bounded by the kernel's native sensor reader.
/// Invalid transport fails the diagnostic; a reported sensor problem continues.
pub fn temperature_record(text:&str,phase:&str,trial:u32)->Result<(),String> {
    let row=text.lines().find(|s|s.starts_with("GT710_SENSOR ")).ok_or("sensor record missing")?;
    let field=|name:&str|row.split_whitespace().find_map(|s|s.strip_prefix(name)).ok_or_else(||format!("sensor field missing: {name}"));
    let number=|name:&str|->Result<Option<u32>,String> {
        match field(name)? {"null"=>Ok(None),n=>n.parse().map(Some).map_err(|_|format!("invalid sensor number: {name}"))}
    };
    let c=number("celsius=")?;let raw=number("raw=")?;let down=number("down_clock=")?.ok_or("threshold missing")?;
    let status=field("status=")?;
    if !status.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'-') {return Err("invalid sensor status".into());}
    let json=|v:Option<u32>|v.map_or_else(||String::from("null"),|v|v.to_string());
    println!("GT710_TEMPERATURE {{\"phase\":\"{phase}\",\"trial\":{trial},\"celsius\":{},\"raw\":{},\"status\":\"{status}\",\"down_clock_c\":{down}}}",json(c),json(raw));
    if c.is_some_and(|c|c>=down) {return Err(format!("down-clock temperature reached: {} >= {down} C",c.unwrap()));}
    Ok(())
}
