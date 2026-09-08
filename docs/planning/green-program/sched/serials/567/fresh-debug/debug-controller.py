import subprocess, json, time, re, os
from pathlib import Path
root=Path('<repo>');tmp=Path('<lane-tmp>')
wrapper=tmp/'debug-tools/gdb_session.sh'
os.chdir(root)
def cmd(s):
    out=subprocess.check_output(['bash',str(wrapper),'cmd',s],text=True)
    print(out,flush=True)
    return json.loads(out.strip().splitlines()[-1])
try:
    print(subprocess.check_output(['git','rev-parse','HEAD'],text=True),flush=True)
    print(subprocess.check_output(['sha256sum','kernel/src/task/boot_resume_oracle.rs'],text=True),flush=True)
    print(subprocess.check_output(['uptime'],text=True),flush=True)
    with (tmp/'fresh-start.log').open('w') as output:
        result=subprocess.run(['bash',str(wrapper),'start'],stdout=output,stderr=subprocess.STDOUT)
    print((tmp/'fresh-start.log').read_text(),flush=True)
    result.check_returncode()
    cmd('set language c')
    r=cmd("info address kernel::task::boot_resume_oracle::run")
    a=int(re.search(r'0x[0-9a-f]+',r['output'])[0],16)
    offset=a if a < 0x10000000000 else a-0x10000000000
    a=0x10000000000+offset
    cmd('hbreak *'+hex(a));cmd('hbreak *'+hex(0x8000000000+offset))
    cmd('continue&')
    serial=tmp/'fresh-gdb-serial.log'
    deadline=time.monotonic()+1000
    while time.monotonic()<deadline:
        s=serial.read_text(errors='replace') if serial.exists() else ''
        if '[TEST:network:loopback_wake_loss_counters_are_zero:PASS]' in s: break
        time.sleep(2)
    else: raise RuntimeError('did not reach oracle window')
    cmd('interrupt')
    sync=cmd('resync-symbols')
    cmd('set $capturebase='+sync['base'])
    cmd('delete breakpoints')
    cmd("disassemble 'kernel::interrupts::context_switch::save_kthread_context'")
    cmd("disassemble 'kernel::interrupts::context_switch::setup_kernel_thread_return'")
    cmd('source <lane-tmp>/fresh-capture.py')
    cmd('continue&')
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        s=serial.read_text(errors='replace')
        if '[BOOT_RESUME_ORACLE:' in s: break
        time.sleep(1)
    else: raise RuntimeError('oracle result absent')
    cmd('interrupt');cmd('serial')
    markers=re.findall(r'\[BOOT_RESUME_ORACLE:[^\]]+\]',s)
    print('ORACLE='+str(markers),flush=True)
    assert markers == ['[BOOT_RESUME_ORACLE:x86:cycles=32:mismatch=1:FAIL]'], markers
    rows=[json.loads(l) for l in (tmp/'fresh-snapshots.jsonl').read_text().splitlines()]
    assert sum(r['event']=='injected_witness' for r in rows)==1
    assert sum(r['event']=='resumed' for r in rows)>=32
    assert not any(r.get('differences') for r in rows)
    print('MUTATION_VERIFIED=1',flush=True)
finally:
    print(subprocess.check_output(['bash',str(wrapper),'stop'],text=True),flush=True)
