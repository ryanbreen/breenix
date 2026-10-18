//! Host-driven USB pointer check. Requests are inputs; the production graphics
//! syscall supplies the observations. Intended for Parallels and VMware.
use libbreenix::graphics;
use std::time::{Duration, Instant};

fn main() {
    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(error) => {
            println!("USB POINTER FAIL framebuffer: {error:?}");
            std::process::exit(1);
        }
    };
    let (width, height) = (info.width as u32, info.height as u32);
    let mut a = (width / 4, height / 3);
    let mut b = (width * 3 / 4, height * 2 / 3);
    let current = match graphics::mouse_state_with_scroll() {
        Ok(state) => state,
        Err(error) => {
            println!("USB POINTER FAIL syscall: {error:?}");
            std::process::exit(1);
        }
    };
    if current.0.abs_diff(a.0) <= 1 && current.1.abs_diff(a.1) <= 1 {
        std::mem::swap(&mut a, &mut b);
    }
    let cases = [
        ("absolute-a", a, 0, 0),
        ("left-press", a, 1, 0),
        ("left-release", a, 0, 0),
        ("absolute-b", b, 0, 0),
        ("right-press", b, 2, 0),
        ("right-release", b, 0, 0),
        ("middle-press", b, 4, 0),
        ("middle-release", b, 0, 0),
        ("wheel-up", b, 0, 1),
        ("wheel-down", b, 0, -1),
    ];
    let mut failed = 0;
    for (seq, (name, (x, y), buttons, wheel)) in cases.iter().enumerate() {
        // Drain the prior case's edge/wheel latches before requesting new input.
        if let Err(error) = graphics::mouse_state_with_scroll() {
            println!("USB POINTER FAIL syscall: {error:?}");
            std::process::exit(1);
        }
        println!("USB POINTER HOST seq={seq} x={x} y={y} buttons={buttons} wheel={wheel}");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observed = (0, 0, 0, 0);
        let mut scroll = 0i32;
        let mut passed = false;
        while Instant::now() < deadline {
            match graphics::mouse_state_with_scroll() {
                Ok(state) => {
                    observed = state;
                    scroll += state.3;
                }
                Err(error) => {
                    println!("USB POINTER FAIL syscall: {error:?}");
                    std::process::exit(1);
                }
            }
            // Absolute USB axes and RFB pixels have separate integer ranges;
            // their two integer scale conversions can differ by one pixel.
            if observed.0.abs_diff(*x) <= 1
                && observed.1.abs_diff(*y) <= 1
                && observed.2 == *buttons
                && scroll == *wheel
            {
                passed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !passed {
            failed += 1;
        }
        println!(
            "USB POINTER CASE {name} {} observed={},{},{} wheel={scroll}",
            if passed { "PASS" } else { "FAIL" },
            observed.0,
            observed.1,
            observed.2
        );
    }
    println!(
        "USB POINTER DONE passed={} failed={failed}",
        cases.len() - failed
    );
    // A disposable native-VM image may run this program as PID 1. Keep its
    // final result available until the host stops that owned VM; a probe-run
    // child exits normally so probe can score its exit status.
    if std::process::id() == 1 {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
