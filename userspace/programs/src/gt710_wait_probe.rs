//! Native CE lifetime check; only the native suite probe exposes these root files.
use libbreenix::{process, signal, time};
fn now_us()->u64 {
    let t=time::now_monotonic().expect("native wait clock");
    t.tv_sec as u64*1_000_000+t.tv_nsec as u64/1000
}
pub fn run() {
    let ready=std::fs::read_to_string("/proc/gt710-ce").unwrap_or_default();
    if ready=="unavailable\n" || ready.is_empty() {return;}
    assert_eq!(ready,"ready\n");
    let pid=match process::fork().expect("native wait fork") {
        process::ForkResult::Child=>{
            let _=std::fs::read_to_string("/proc/gt710-wait");
            process::exit(99);
        }
        process::ForkResult::Parent(pid)=>pid.raw() as i32,
    };
    let end=now_us()+2_000_000;
    loop {
        if std::fs::read_to_string("/proc/gt710-ce").expect("native status")=="waiting held=1 completed=0\n" {break;}
        let mut status=0;
        match process::waitpid(pid,&mut status,1) {
            Ok(got) if got.raw() as i32==pid=>{
                println!("GT710_KILLED_WAIT_ERROR child exited before queue status={}",status);
                return;
            }
            Err(error)=>{
                println!("GT710_KILLED_WAIT_ERROR child status {:?}",error);
                return;
            }
            _=>(),
        }
        if now_us()>=end {
            let _=signal::kill(pid,signal::SIGKILL);
            let mut status=0;
            let _=process::waitpid(pid,&mut status,0);
            println!("GT710_KILLED_WAIT_ERROR native CE waiter never queued");
            return;
        }
        time::sleep_ms(1).unwrap();
    }
    let start=now_us();
    signal::kill(pid,signal::SIGKILL).expect("kill native waiter");
    let mut status=0;
    loop {
        let got=process::waitpid(pid,&mut status,1).expect("reap native waiter");
        if got.raw() as i32==pid {break;}
        assert!(now_us()-start<=250_000,"native wait kill exceeds 250 ms");
        time::sleep_ms(1).unwrap();
    }
    let kill_us=now_us()-start;
    assert!(kill_us<=250_000 && status&127==signal::SIGKILL as i32,"native wait did not exit by SIGKILL");
    assert_eq!(std::fs::read_to_string("/proc/gt710-ce").unwrap(),"waiting held=1 completed=0\n");
    let receipt=std::fs::read_to_string("/proc/gt710-drain").expect("native deferred drain");
    assert_eq!(receipt,"drained compares=2 survivor=1 baseline=1\n");
    println!("GT710_KILLED_WAIT {{\"schema\":1,\"waiter\":{},\"held_before\":1,\"held_after_kill\":1,\"released_before_drain\":0,\"killed\":true,\"kill_us\":{},\"drained\":true,\"after_baseline\":true,\"survivor\":true,\"device_completed_before_kill\":false,\"complete_compares\":2,\"poisons\":[85,170]}}",pid,kill_us);
    let recovery=std::fs::read_to_string("/proc/gt710-recovery");
    match recovery {
        Ok(record)=>print!("{}",record),
        Err(error)=>println!("GT710_RECOVERY_ERROR {}",error),
    }
}
