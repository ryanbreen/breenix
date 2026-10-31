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
    let batches=if kind=="gpc" {8}else{1};
    let begin = time::now_monotonic().map_err(|e| format!("monotonic begin: {e:?}"))?.as_nanos();
    for _ in 0..batches {
    let (result, submit) = gpu::submit(context, words);
    checked(result, "SUBMIT")?;
    if submit.fence == 0 { return Err(String::from("SUBMIT returned no fence")); }
    // Each bounded batch uses the production fence. No per-draw wait or readback.
    let (result, wait) = gpu::wait(context, submit.fence, 30_000_000_000);
    checked(result, "WAIT")?;
    if wait.status != fence_status::OK {
        return Err(format!("WAIT status={} answer={:#x}", wait.status, wait.answer));
    }
    }
    let end = time::now_monotonic().map_err(|e| format!("monotonic end: {e:?}"))?.as_nanos();
    let elapsed = end.checked_sub(begin).filter(|v| *v > 0).ok_or("monotonic interval did not advance")?;
    println!("GT710_BENCH {{\"kind\":\"{kind}\",\"trial\":{trial},\"begin_ns\":{begin},\"end_ns\":{end},\"elapsed_ns\":{elapsed},\"work\":{work},\"batches\":{batches},\"unit\":\"{unit}\",\"completion\":true}}");
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
    let mut registers=Vec::new();
    for pair in field("registers=")?.split(',') {
        let (a,v)=pair.split_once(':').ok_or("malformed sensor register")?;
        let a=a.parse::<u32>().map_err(|_|"invalid sensor register address")?;
        let v=v.parse::<u32>().map_err(|_|"invalid sensor register value")?;
        registers.push(format!("[{a},{v}]"));
    }
    let registers=registers.join(",");
    let json=|v:Option<u32>|v.map_or_else(||String::from("null"),|v|v.to_string());
    println!("GT710_TEMPERATURE {{\"phase\":\"{phase}\",\"trial\":{trial},\"celsius\":{},\"raw\":{},\"status\":\"{status}\",\"down_clock_c\":{down},\"registers\":[{registers}]}}",json(c),json(raw));
    if c.is_some_and(|c|c>=down) {return Err(format!("down-clock temperature reached: {} >= {down} C",c.unwrap()));}
    Ok(())
}

/// C3-2: settled solid control and shader-heavy gradient at POST/low/A/C/D.
/// The observer sets and verifies at n=0 and reads temperature before each trial.
pub fn run_c3(mut observe:impl FnMut(&str,u32)->Result<(),String>)->Result<(),String> {
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
    // GRADIENT_V executes four fixed-step wide integer divisions per fragment.
    // The small target has 16 KiB of colour storage; the shader has no textures
    // or local-memory spills. Sixteen draws exactly fill the per-submit WORK
    // bound (65536 fragments * 32768). Eight batches amortize host overhead.
    let gpc_target=gpu::target_create(format::B8G8R8A8,64,64).map_err(|e|format!("GPC target: {e}"))?;
    let mut gpc=Vec::new();encode::begin(&mut gpc,gpc_target);
    let mut params=[0;16];params[..3].copy_from_slice(&[0,64,0xffe0c080]);
    for _ in 0..16 {
        gpu::abi::draw::encode(&mut gpc,pipeline::GRADIENT_V,0xff204080,0,0,0,
            &[gpu::abi::draw::Instance {rect:[0,0,64,64],params}]);
    }
    let mut n=0;
    for stage in ["post","low","A","C","D"] {
        observe(stage,0)?;
        println!("GT710_C3_STAGE stage={stage}");
        if stage=="post" {
            for _ in 0..6 {
                n+=1;observe(stage,n)?;
                trial(context,&draws,"core",n,u64::from(DRAWS)*u64::from(WIDTH)*u64::from(HEIGHT),"fragments")?;
            }
        }
        for _ in 0..3 {
            n+=1;observe(stage,n)?;
            trial(context,&gpc,"gpc",n,8*16*64*64,"fragments")?;
        }
        if stage=="C" {
            for _ in 0..3 {
                n+=1;observe(stage,n)?;
                trial(context,&draws,"core",n,u64::from(DRAWS)*u64::from(WIDTH)*u64::from(HEIGHT),"fragments")?;
            }
        }
    }
    checked(gpu::resource_destroy(gpc_target),"GPC RESOURCE_DESTROY")?;
    checked(gpu::resource_destroy(target),"RESOURCE_DESTROY")?;
    checked(gpu::context_destroy(context),"CONTEXT_DESTROY")
}

/// C4 runs the existing two workloads after source-defined PMU ring readiness.
pub fn run_c4(observe:impl FnMut(&str,u32)->Result<(),String>)->Result<(),String> {
    run_clock_workloads(observe,&["post"])
}
/// K5 uses the identical workloads at baseline, target and restored memory.
pub fn run_c5e(observe:impl FnMut(&str,u32)->Result<(),String>)->Result<(),String> {
    run_clock_workloads(observe,&["baseline","target","return"])
}
fn run_clock_workloads(mut observe:impl FnMut(&str,u32)->Result<(),String>,stages:&[&str])->Result<(),String> {
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
    // GRADIENT_V executes four fixed-step wide integer divisions per fragment.
    // The small target has 16 KiB of colour storage; the shader has no textures
    // or local-memory spills. Sixteen draws exactly fill the per-submit WORK
    // bound (65536 fragments * 32768). Eight batches amortize host overhead.
    let gpc_target=gpu::target_create(format::B8G8R8A8,64,64).map_err(|e|format!("GPC target: {e}"))?;
    let mut gpc=Vec::new();encode::begin(&mut gpc,gpc_target);
    let mut params=[0;16];params[..3].copy_from_slice(&[0,64,0xffe0c080]);
    for _ in 0..16 {
        gpu::abi::draw::encode(&mut gpc,pipeline::GRADIENT_V,0xff204080,0,0,0,
            &[gpu::abi::draw::Instance {rect:[0,0,64,64],params}]);
    }
    observe("pmu",0)?;
    for &stage in stages {
        println!("GT710_CLOCK_BENCH_STAGE phase={stage}");
        observe(stage,0)?;
        for n in 1..=3 {
            observe(stage,n)?;
            trial(context,&draws,"core",n,u64::from(DRAWS)*u64::from(WIDTH)*u64::from(HEIGHT),"fragments")?;
        }
        for n in 4..=6 {
            observe(stage,n)?;
            trial(context,&gpc,"gpc",n,8*16*64*64,"fragments")?;
        }
        observe(stage,7)?;
    }
    checked(gpu::resource_destroy(gpc_target),"GPC RESOURCE_DESTROY")?;
    checked(gpu::resource_destroy(target),"RESOURCE_DESTROY")?;
    checked(gpu::context_destroy(context),"CONTEXT_DESTROY")
}

/// Fresh console output plus the production native shadow/screen readback.
pub fn console_c5e(phase:&str)->Result<(),String> {
    use std::io::Write;
    let marker=format!("GT710-C5E-{phase}-FRESH-Wm09");
    let mut console=std::fs::OpenOptions::new().write(true).open("/dev/console").map_err(|e|format!("console open: {e}"))?;
    console.write_all(format!("\r\n{marker}\r\n").as_bytes()).map_err(|e|format!("console write: {e}"))?;
    for _ in 0..20 {
        let text=std::fs::read_to_string("/proc/console/check").map_err(|e|format!("native console readback: {e}"))?;
        if text.lines().any(|s|s.starts_with("console row: ") && s.contains(&marker)) {
            for row in text.lines().filter(|s|s.starts_with("console check:") || s.starts_with("console frame:") || s.starts_with("console scanout:") || s.contains(&marker)) {println!("GT710_C5E_CONSOLE phase={phase} {row}");}
            if !text.lines().any(|s|s=="console check: shadow and screen identical") || !text.lines().any(|s|s.starts_with("console frame: ") && s.ends_with(" complete")) {
                return Err("C5e console readback mismatch".into());
            }
            println!("GT710_C5E_CONSOLE_OK phase={phase} fresh=true identical=true");return Ok(());
        }
        time::sleep_ms(100).map_err(|e|format!("console flush wait: {e:?}"))?;
    }
    Err("C5e fresh console row absent from native readback".into())
}
