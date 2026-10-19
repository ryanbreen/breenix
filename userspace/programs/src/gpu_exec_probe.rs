//! Fresh-image checks for the GPU suite's exec-reclamation case.
use libbreenix::{gpu, io, process, time};
use gpu::abi::{self, context_state};
#[path = "suite_gpu/refusals.rs"]
mod refusals;
use refusals::refusals_for;
use libbreenix::types::Fd;

fn write_all(fd: Fd, mut bytes: &[u8]) -> Result<(), String> {
    while !bytes.is_empty() {
        match io::write(fd, bytes) {
            Ok(n) if n > 0 => bytes = &bytes[n..],
            _ => return Err("the pipe to the parent closed".into()),
        }
    }
    Ok(())
}

fn send(fd: Fd, tag: u8, payload: &[u8]) -> Result<(), String> {
    let mut header = [tag, 0, 0, 0, 0];
    header[1..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    write_all(fd, &header)?;
    write_all(fd, payload)
}

fn probe(rd: Fd, wr: Fd, handles: &[u64]) -> Result<(), String> {
    let (r, k) = gpu::counters(0);
    if r != 0 || k.contexts != 0 || k.resources != 0 || k.resource_bytes != 0 {
        return Err(format!("after exec COUNTERS returned {r}: {} contexts, {} resources, {} resource bytes", k.contexts, k.resources, k.resource_bytes));
    }
    let ctx = gpu::context_create().map_err(|e| format!("CONTEXT_CREATE: {e}"))?;
    loop {
        let (r, k) = gpu::counters(ctx);
        if r != 0 { return Err(format!("COUNTERS of the new context: {r}")); }
        match k.state {
            context_state::LIVE => break,
            context_state::WARMING => { let _ = time::sleep_ms(5); }
            state => return Err(format!("the new context's state became {state}")),
        }
    }
    for &handle in handles {
        for (op, r) in refusals_for(handle, ctx) {
            if r != -abi::errno::EINVAL {
                return Err(format!("{op} with a handle from before exec ({handle:#018x}) returned {r}, expected EINVAL"));
            }
        }
    }
    if gpu::context_destroy(ctx) != 0 { return Err("CONTEXT_DESTROY of the probe's context".into()); }
    send(wr, b'X', &[])?;
    let mut reply = [0; 5];
    let mut done = 0;
    while done < reply.len() {
        match io::read(rd, &mut reply[done..]) {
            Ok(n) if n > 0 => done += n,
            _ => return Err("the pipe from the parent closed".into()),
        }
    }
    if reply != [b'Q', 0, 0, 0, 0] { return Err("expected the parent's Q message".into()); }
    Ok(())
}


fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 { process::exit(2); }
    let rd = args[0].parse::<u64>().unwrap_or_else(|_| process::exit(2));
    let wr = args[1].parse::<u64>().unwrap_or_else(|_| process::exit(2));
    let handles: Vec<u64> = args[2..].iter().map(|s| u64::from_str_radix(s, 16).unwrap_or_else(|_| process::exit(2))).collect();
    let rd = Fd::from_raw(rd);
    let wr = Fd::from_raw(wr);
    let result = probe(rd, wr, &handles);
    let _ = match result {
        Ok(()) => send(wr, b'K', &[]),
        Err(message) => send(wr, b'F', message.as_bytes()),
    };
    process::exit(0);
}
