use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use crate::CmdResult;

pub fn threads() -> CmdResult {
    let handles: Vec<_> = (0..4u64)
        .map(|k| thread::spawn(move || (k * 1000..(k + 1) * 1000).sum::<u64>()))
        .collect();
    let mut total = 0;
    for handle in handles {
        total += handle.join().map_err(|_| "a thread panicked")?;
    }
    println!("threads=4 total={total}");
    let named = thread::Builder::new()
        .name("worker".into())
        .spawn(|| thread::current().name().map(String::from))
        .map_err(|e| format!("spawn a named thread: {e}"))?;
    println!("name={:?}", named.join().map_err(|_| "the named thread panicked")?);
    Ok(())
}

pub fn sync() -> CmdResult {
    let counter = Arc::new(Mutex::new(0u64));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..10_000 {
                    *counter.lock().unwrap() += 1;
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().map_err(|_| "a counting thread panicked")?;
    }
    println!("counter={}", *counter.lock().unwrap());
    let (tx, rx) = mpsc::channel();
    let sender = thread::spawn(move || {
        for i in 1..=100u64 {
            tx.send(i).unwrap();
        }
    });
    let sum: u64 = rx.iter().sum();
    sender.join().map_err(|_| "the sending thread panicked")?;
    println!("channel sum={sum}");
    Ok(())
}
