//! TCP dup'd-listener survival test (std version)
//!
//! Regression test for #724 review finding M1: `dup()`/`dup2()` on a
//! TcpListener fd never called `tcp_listener_ref_inc`, so a listener with
//! two live fds (the original plus a dup) still had `ref_count == 1`. When
//! #724's fix made `sys_close()` decrement that count on close, closing
//! EITHER fd hit `old_count == 1` and retired the listener
//! (`listeners.remove(&port)`) while the OTHER fd -- which the caller still
//! believes is a live, working listener -- was left pointing at a port that
//! no longer exists.
//!
//! This test binds a listener, `dup()`s it, closes the ORIGINAL fd, and then
//! proves the port is still genuinely listening by connecting to it and
//! accepting through the SURVIVING (dup'd) fd. Under the pre-fix code this
//! either fails outright (accept on a fd whose listener entry has been
//! removed) or a concurrent bind to the same port from elsewhere would
//! wrongly succeed (EADDRINUSE would wrongly NOT fire) -- either way, the
//! surviving fd's protocol contract ("I still own a live listener") is
//! broken by the first close.
//!
//! Marker sequence:
//!   TCP_DUP_LISTENER_TEST_PASSED  -- ref-count symmetry holds; a dup'd
//!                                    listener survives exactly one close,
//!                                    and the SECOND close actually retires
//!                                    it (port becomes free again).
//!   TCP_DUP_LISTENER_TEST_FAILED  -- any step below did not hold.

use std::process;

use libbreenix::error::Error;
use libbreenix::errno::Errno;
use libbreenix::io;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::Fd;

const PORT: u16 = 9110;
const MAX_RETRIES: usize = 10;

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    println!("TCP_DUP_LISTENER_TEST_FAILED");
    process::exit(1);
}

fn accept_with_retry(server_fd: Fd) -> Result<Fd, Error> {
    for retry in 0..MAX_RETRIES {
        match socket::accept(server_fd, None) {
            Ok(fd) => return Ok(fd),
            Err(Error::Os(Errno::EAGAIN)) => {
                if retry < MAX_RETRIES - 1 {
                    for _ in 0..20000 {
                        std::hint::spin_loop();
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::Os(Errno::EAGAIN))
}

/// Attempt a loopback connect+accept round-trip through `listener_fd`.
/// Returns Ok(()) if a connection was accepted, Err otherwise. Always closes
/// both the client and accepted fds it creates (not the listener).
fn connect_and_accept_through(listener_fd: Fd) -> Result<(), Error> {
    let client_fd = socket::socket(AF_INET, SOCK_STREAM, 0)?;
    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], PORT);
    let connect_result = socket::connect_inet(client_fd, &loopback_addr);
    if let Err(e) = connect_result {
        let _ = io::close(client_fd);
        return Err(e);
    }
    let accepted = accept_with_retry(listener_fd);
    let _ = io::close(client_fd);
    match accepted {
        Ok(accepted_fd) => {
            let _ = io::close(accepted_fd);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn main() {
    println!("=== TCP Dup'd Listener Survival Test (#724 review M1) ===");

    // Step 1: create, bind, listen.
    println!("\nStep 1: bind + listen on port {}...", PORT);
    let listener_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  socket() returned error: {:?}", e);
            fail("socket() failed");
        }
    };
    let local_addr = SockAddrIn::new([0, 0, 0, 0], PORT);
    if let Err(e) = socket::bind_inet(listener_fd, &local_addr) {
        println!("  bind() returned error: {:?}", e);
        fail("bind() failed");
    }
    if let Err(e) = socket::listen(listener_fd, 128) {
        println!("  listen() returned error: {:?}", e);
        fail("listen() failed");
    }
    println!("  PASS: bound and listening (fd={})", listener_fd.raw() as i32);

    // Step 2: dup() the listener fd. This is the exact inc-side path M1
    // found missing (dup()/dup2()/F_DUPFD never called tcp_listener_ref_inc).
    println!("\nStep 2: dup() the listener fd...");
    let dup_fd = match io::dup(listener_fd) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  dup() returned error: {:?}", e);
            fail("dup() of listener fd failed");
        }
    };
    if dup_fd == listener_fd {
        fail("dup() returned the same fd as the original");
    }
    println!(
        "  PASS: dup'd listener fd={} (original fd={})",
        dup_fd.raw() as i32,
        listener_fd.raw() as i32
    );

    // Step 3: close the ORIGINAL fd. Under the pre-fix code the listener's
    // ref_count was 1 (dup() never incremented it), so this close hits
    // old_count == 1 and retires the listener out from under the surviving
    // dup'd fd -- exactly the M1 hazard.
    println!("\nStep 3: close the ORIGINAL fd (dup'd fd must survive this)...");
    if let Err(e) = io::close(listener_fd) {
        println!("  close(original) returned error: {:?}", e);
        fail("close() of original listener fd failed");
    }
    println!("  Original fd closed");

    // Step 4: the SURVIVING dup'd fd must still be a live, working listener:
    // a real loopback connect+accept round-trip must succeed through it.
    println!("\nStep 4: connect+accept through the SURVIVING dup'd fd...");
    match connect_and_accept_through(dup_fd) {
        Ok(()) => {
            println!("  PASS: accepted a connection through the dup'd fd after the original closed");
        }
        Err(e) => {
            println!("  connect+accept through dup'd fd returned error: {:?}", e);
            fail(
                "the dup'd listener fd did not survive closing the original fd -- \
                 the listener was retired early (M1 regression)",
            );
        }
    }

    // Step 5: close the SURVIVING (now last) fd too. THIS close must
    // actually retire the listener -- proving the fix is real ref counting,
    // not "never decrement" (which would leak the port forever). A second
    // bind to the same port must now succeed.
    println!("\nStep 5: close the last fd; the listener must now actually retire...");
    if let Err(e) = io::close(dup_fd) {
        println!("  close(dup) returned error: {:?}", e);
        fail("close() of the surviving dup'd fd failed");
    }

    let rebind_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  socket() for rebind check returned error: {:?}", e);
            fail("socket() for rebind check failed");
        }
    };
    match socket::bind_inet(rebind_fd, &local_addr) {
        Ok(()) => {
            println!("  PASS: port {} was free after the last fd closed (listener genuinely retired)", PORT);
            let _ = io::close(rebind_fd);
        }
        Err(e) => {
            println!("  bind() after last close returned error: {:?}", e);
            let _ = io::close(rebind_fd);
            fail(
                "port was still held after the last fd closed -- the listener was never \
                 retired (ref-count leak in the other direction)",
            );
        }
    }

    println!("\n=== All TCP dup'd-listener tests passed! ===");
    println!("TCP_DUP_LISTENER_TEST_PASSED");
    process::exit(0);
}
