use std::sync::Mutex;
type CaseResult = Result<(), String>;
mod kind { pub const KEY_PRESS: u16 = 1; }
#[derive(Clone)]
struct Event { kind: u16, text_len: u32, text: Vec<u8> }
#[derive(Clone, Copy)]
struct State {
    now: u64, ack_at: u64, ticket: u64, marker: &'static str,
    text_at: u64, retry_at: u64, retried: bool, transport_done: bool,
    wait_began: u64, cursor_in_pixel: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    now: 0, ack_at: 0, ticket: 0, marker: "", text_at: 0,
    retry_at: 0, retried: false, transport_done: false,
    wait_began: 0, cursor_in_pixel: true,
});
fn now() -> u64 { STATE.lock().unwrap().now }
fn case_ms_left() -> u64 { 10_000u64.saturating_sub(now() / 1_000_000) }
fn pause() { time::sleep_ms(5).unwrap(); }
fn check(b: bool, why: &str) -> CaseResult { if b { Ok(()) } else { Err(why.into()) } }
mod time {
    pub fn sleep_ms(ms: u64) -> Result<(), String> {
        let mut s = super::STATE.lock().unwrap();
        s.now += ms * 1_000_000;
        if s.retry_at != 0 && s.now >= s.retry_at { s.retried = true; }
        if s.now >= s.ack_at && s.ack_at != 0 {
            s.transport_done = true;
            s.cursor_in_pixel = false;
            // A different ticket, wrong ACK kind and malformed line must not
            // satisfy the wait. The real ACK carries a wire sequence, not seq=7.
            std::fs::write("/proc/kmsg", format!(
                "HOSTINPUT_ACK BXKEY_ACK 000000000000000f ticket=6 ns=1\n\
                 HOSTINPUT_ACK BXPTR_ACK 000000000000001f ticket=8 ns=1\n\
                 HOSTINPUT_ACK {} bad ticket=7 ns=1\n\
                 HOSTINPUT_ACK {} 0000000000000020 ticket={} ns={}\n",
                s.marker, s.marker, s.ticket, s.ack_at)).unwrap();
        }
        Ok(())
    }
}
pub mod breenix_suite {
    pub mod suite {
        use super::super::STATE;
        pub fn hostkey_method() -> Option<&'static str> { Some("host-channel") }
        mod host_input {
            pub fn keys(_: &str, _: impl FnOnce() -> Result<u64,String>) -> Result<u64,String> { panic!("native path on serial") }
            pub fn pointer(_: u32,_: u32,_: u32,_: u32,_: u32,_: i32,_: impl FnOnce() -> Result<u64,String>) -> Result<u64,String> { panic!("native path on serial") }
        }
        pub fn case_ms_left() -> u64 { super::super::case_ms_left() }
        pub mod time { pub use super::super::super::time::sleep_ms; }
        pub fn hostkeys(chords: &str) -> Result<u64, String> { hostkeys_request(chords, None) }
        pub fn hostkeys_request(chords: &str, ticket: Option<u64>) -> Result<u64, String> {
            assert_eq!(chords, "shift-h,e,l,l,o,space,shift-w,o,r,l,d");
            let mut s = STATE.lock().unwrap();
            s.marker = "BXKEY_ACK"; s.ticket = ticket.unwrap_or(0);
            // Eleven ordered chords, with a retry at the driver's 350 ms timer.
            s.retry_at = 350_000_000; s.ack_at = 2_150_000_000;
            s.text_at = s.ack_at + 20_000_000;
            Ok(7)
        }
        pub fn hostpointer_request(x: u32, y: u32, w: u32, h: u32, buttons: u32, wheel: i32, ticket: Option<u64>) -> Result<u64, String> {
            assert_eq!((x,y,w,h,buttons,wheel), (2520,1500,2560,1600,0,0));
            let mut s = STATE.lock().unwrap();
            s.marker = "BXPTR_ACK"; s.ticket = ticket.unwrap_or(0);
            // The retained failure's 202 ms park delay exceeds the old sleep.
            s.ack_at = 202_000_000;
            Ok(7)
        }
        pub fn host_ack_ticket() -> Result<u64, String> { Ok(7) }
        pub fn monotonic_ns() -> Option<i128> { Some(i128::from(super::super::now())) }
        // HELPERS
    }
}
use breenix_suite::suite::hostkeys;
pub struct Desktop { width: u32, height: u32 }
impl Desktop {
    pub fn pointer(&self, x: i32, y: i32, buttons: u32, wheel: i32) -> CaseResult {
        // The old async enqueue boundary, matching Desktop::pointer's contract.
        let _ = (x,y,buttons,wheel);
        let mut s = STATE.lock().unwrap(); s.ack_at = 202_000_000; Ok(())
    }
    fn focus(&mut self, _: &mut Client) -> CaseResult { Ok(()) }
}
struct Client { events: Vec<Event> }
impl Client {
    fn clear(&mut self) -> CaseResult { self.events.clear(); Ok(()) }
    fn snapshot(&mut self) -> CaseResult {
        let mut s = STATE.lock().unwrap();
        if s.wait_began == 0 { s.wait_began = s.now; }
        if s.now >= s.text_at {
            self.events = vec![Event { kind: kind::KEY_PRESS, text_len: 11, text: b"Hello World".to_vec() }];
        }
        Ok(())
    }
}
fn one() -> Result<(Desktop, Client), String> { Ok((Desktop { width:2560, height:1600 }, Client {events:Vec::new()})) }
// DEADLINE
// PARKED
// TEXT_EXACT
fn reset() {
    *STATE.lock().unwrap() = State {
        now:0, ack_at:0, ticket:0, marker:"", text_at:0, retry_at:0,
        retried:false, transport_done:false, wait_began:0, cursor_in_pixel:true,
    };
    std::fs::write("/proc/kmsg", "HOSTINPUT_ACK BXPTR_ACK 000000000000001f ticket=8 ns=1\n").unwrap();
}
#[test]
fn park_ack_later_than_150_ms_keeps_cursor_out_of_checked_pixel() {
    reset();
    let (d, _) = one().unwrap();
    parked(&d).unwrap();
    let s = *STATE.lock().unwrap();
    assert!(!s.cursor_in_pixel, "scanout pixel still contains the cursor before its own parking ACK");
    assert!(s.transport_done);
    assert!(s.now >= 202_000_000);
}
#[test]
fn key_resend_before_last_ack_does_not_consume_the_delivery_bound() {
    reset();
    let result = text_exact();
    let s = *STATE.lock().unwrap();
    assert!(s.retried, "fixture must resend before the final ACK");
    assert_eq!(result, Ok(()), "serial retry consumed bwm's 2000 ms delivery window");
    assert_eq!(s.wait_began, s.ack_at, "delivery window must begin at the last packet ACK");
}
