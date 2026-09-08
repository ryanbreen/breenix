from pathlib import Path
import subprocess,sys
reg=Path('kernel/src/test_framework/registry.rs');tcp=Path('kernel/src/net/tcp.rs')
reg.write_bytes(subprocess.check_output(['git','show','HEAD:'+str(reg)]));tcp.write_bytes(subprocess.check_output(['git','show','HEAD:'+str(tcp)]))
mode=sys.argv[1]
if mode in ['wake-defect','always-extend','delayed-guard','delayed-always']:
 s=tcp.read_text();s=s.replace('fn wake_connection_waiters(conn: &TcpConnection) {','fn wake_connection_waiters(conn: &TcpConnection) {\n    if conn.id.local_port == 54_511 { return; }',1);tcp.write_text(s)
if mode in ['always-extend','delayed-always']:
 s=reg.read_text();start=s.index('fn loopback_extension_eligible(');end=s.index('\n}',start)+2
 s=s[:start]+'''fn loopback_extension_eligible(pending: bool, extensions: u64) -> bool {
    pending && extensions < LOOPBACK_WAKE_MAX_EXTENSIONS
}'''+s[end:]
 s=s.replace('        let starved = loopback_window_starved(tick_ms, ctr_ms, switches);\n','')
 start=s.index('        let runnable = matches!(',s.index('fn run_loopback_recv_wake_test_inner'))
 end=s.index('        if loopback_extension_eligible',start)
 s=s[:start]+s[end:];s=s.replace('loopback_extension_eligible(pending, runnable, starved, extensions)','loopback_extension_eligible(pending, extensions)');reg.write_text(s)
if mode=='extension-deleted':
 s=reg.read_text();start=s.index('fn loopback_extension_eligible(');end=s.index('\n}',start)+2;s=s[:start]+s[end:]
 s=s.replace('    let mut extensions = 0u64;', '    let extensions = 0u64;')
 s=s.replace('        let starved = loopback_window_starved(tick_ms, ctr_ms, switches);\n','').replace('        let pending = wake_ms == 0;\n','')
 start=s.index('        let runnable = matches!(',s.index('fn run_loopback_recv_wake_test_inner'));end=s.index('        break (tick_ms',start);s=s[:start]+s[end:]
 # Keep no loop that can only break once.
 s=s.replace('let (elapsed_tick_ms, elapsed_ctr_ms, ctx_delta, wake_ms, reader_state) = loop {','let (elapsed_tick_ms, elapsed_ctr_ms, ctx_delta, wake_ms, reader_state) = {').replace('        break (tick_ms, ctr_ms, switches, wake_ms, reader_state);','        (tick_ms, ctr_ms, switches, wake_ms, reader_state)')
 reg.write_text(s)

if mode in ['delayed-guard', 'delayed-always']:
 s=reg.read_text()
 anchor='    // Measure each spent window independently using guest ticks, counter time,'
 injection="""    // R232 mutation: defer the suppressed connection wake for 600 ms.
    let delayed_wake = kthread::kthread_run(move || {
        sleep_current_thread_ms(600);
        let _ = scheduler::wake_thread_any_context(reader_tid);
    }, "r232-delayed-wake").expect("delayed wake injection thread");

"""
 s=s.replace(anchor,injection+anchor,1)
 anchor='    // Latch the deadline wake stamp: a dispatch after refusal cannot erase red.'
 s=s.replace(anchor,'    let _ = kthread::kthread_join(&delayed_wake);\n\n'+anchor,1)
 reg.write_text(s)
