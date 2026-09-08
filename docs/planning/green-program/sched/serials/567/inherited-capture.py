import gdb, json, struct
OUT = '<lane-tmp>/loopback-snapshots-v2.jsonl'
BASE = int(gdb.parse_and_eval('$capturebase'))
REGS = 'r15 r14 r13 r12 r11 r10 r9 r8 rdi rsi rbp rbx rdx rcx rax'.split()
CTX = 'rax rbx rcx rdx rsi rdi rbp rsp r8 r9 r10 r11 r12 r13 r14 r15 rip rflags cs ss'.split()
latest = None
serial = 0
pending = False

def reg(name):
    return int(gdb.parse_and_eval('$'+name)) & ((1<<64)-1)

def words(addr, n):
    return list(struct.unpack('<'+'Q'*n, gdb.selected_inferior().read_memory(addr,8*n)))

def emit(event, **data):
    with open(OUT,'a') as f: f.write(json.dumps(dict(event=event, **data))+'\n')

def frame(addr):
    return dict(zip(['rip','cs','rflags','rsp','ss'],words(addr,5)))

class Save(gdb.Breakpoint):
    def stop(self):
        global latest, serial, pending
        if reg('rdi') != 1: return False
        serial += 1
        f=frame(reg('rdx'))
        latest=dict(zip(REGS,words(reg('rsi'),15))); latest.update(f)
        pending=True
        emit('switch_away', sequence=serial, frame_slot=reg('rdx'), regs_slot=reg('rsi'), snapshot_slot=words(reg('gs_base')+24,1)[0]+0x130, state=latest, stack=words(f['rsp'],16))
        return False

class Restore(gdb.Breakpoint):
    def stop(self):
        if reg('rdi') != 1 or latest is None: return False
        slot=words(reg('gs_base')+24,1)[0]+0x130
        state=dict(zip(CTX,words(slot,20)))
        diff={k:[latest[k],state[k]] for k in CTX if latest[k]!=state[k]}
        emit('restore_input', sequence=serial, snapshot_slot=slot, output_frame_slot=reg('rdx'), state=state, differences=diff, stack=words(state['rsp'],16))
        return bool(diff)

class Resume(gdb.Breakpoint):
    def __init__(self, state, sequence):
        super().__init__('*'+hex(state['rip']),internal=True,temporary=True)
        self.state=state.copy(); self.sequence=sequence
    def stop(self):
        self.enabled = False
        gdb.post_event(self.delete)
        state={k:reg('eflags' if k=='rflags' else k) for k in CTX}
        diff={k:[self.state[k],state[k]] for k in CTX if self.state[k]!=state[k]}
        emit('resumed',sequence=self.sequence,state=state,differences=diff)
        return bool(diff)

class Iret(gdb.Breakpoint):
    def stop(self):
        global pending
        if not pending: return False
        gs=reg('gs_base')
        if words(gs+8,1)!=words(gs+24,1): return False
        actual=frame(reg('rsp'))
        actual.update({k:reg(k) for k in REGS})
        actual['rflags'] |= 2
        diff={k:[latest[k],actual[k]] for k in CTX if latest[k]!=actual[k]}
        emit('iret_frame',sequence=serial,frame_slot=reg('rsp'),state=actual,differences=diff)
        pending=False
        if not diff: Resume(latest,serial)
        return bool(diff)

Save('*'+hex(BASE+0x2fa2e0),internal=True)
Restore('*'+hex(BASE+0x2fe0b0),internal=True)
Iret('*'+hex(BASE+0x3fd16d),internal=True)
emit('capture_start',base=BASE,save_offset=0x2fa2e0,restore_offset=0x2fe0b0,iret_offset=0x3fd16d,context_offset=0x130)
