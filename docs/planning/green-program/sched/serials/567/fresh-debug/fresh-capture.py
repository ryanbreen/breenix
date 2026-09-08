import gdb, json, struct, re
OUT='<lane-tmp>/fresh-snapshots.jsonl'
REGS='r15 r14 r13 r12 r11 r10 r9 r8 rdi rsi rbp rbx rdx rcx rax'.split()
CTX='rax rbx rcx rdx rsi rdi rbp rsp r8 r9 r10 r11 r12 r13 r14 r15 rip rflags cs ss'.split()
latest=None
sequence=0
injected=False

def addr(name):
    s=gdb.execute("info address "+name,to_string=True)
    value=int(re.search(r'0x[0-9a-f]+',s)[0],16)
    base=int(gdb.parse_and_eval('$capturebase'))
    return value+base if value < base else value

def reg(name): return int(gdb.parse_and_eval('$'+name)) & ((1<<64)-1)
def words(a,n): return list(struct.unpack('<'+'Q'*n,gdb.selected_inferior().read_memory(a,8*n)))
def emit(event,**kw):
    with open(OUT,'a') as f: f.write(json.dumps(dict(event=event,**kw))+'\n')
def frame(a): return dict(zip('rip cs rflags rsp ss'.split(),words(a,5)))
witness=addr('kernel::task::boot_resume_oracle::witness_resume')
insns=gdb.selected_frame().architecture().disassemble(witness,witness+350)
resume=next(insns[i+1]['addr'] for i,x in enumerate(insns) if x['asm']=='hlt')
save=addr('kernel::interrupts::context_switch::save_kthread_context')
restore=addr('kernel::interrupts::context_switch::setup_kernel_thread_return')
emit('addresses',base=int(gdb.parse_and_eval('$capturebase')),witness=witness,resume=resume,save=save,restore=restore,disassembly=insns,save_entry=gdb.selected_frame().architecture().disassemble(save,save+128),restore_entry=gdb.selected_frame().architecture().disassemble(restore,restore+128))

class Save(gdb.Breakpoint):
    def stop(self):
        global latest,sequence
        f=frame(reg('rdx'))
        if reg('rdi')!=1 or f['rip']!=resume: return False
        sequence+=1
        latest=dict(zip(REGS,words(reg('rsi'),15)));latest.update(f)
        emit('switch_away',sequence=sequence,state=latest,frame_slot=reg('rdx'),regs_slot=reg('rsi'),thread=words(reg('gs_base')+24,1)[0])
        return False

class Restore(gdb.Breakpoint):
    def stop(self):
        if reg('rdi')!=1 or latest is None: return False
        thread=words(reg('gs_base')+24,1)[0]
        slot=thread+0x130
        state=dict(zip(CTX,words(slot,20)))
        if state['rip']!=resume: return False
        diff={k:[latest[k],state[k]] for k in CTX if latest[k]!=state[k]}
        emit('restore_input',sequence=sequence,snapshot_slot=slot,output_frame_slot=reg('rdx'),state=state,differences=diff)
        return bool(diff)

class Resume(gdb.Breakpoint):
    def stop(self):
        global injected
        if latest is None: return False
        state={k:reg('eflags' if k=='rflags' else k) for k in CTX}
        diff={k:[latest[k],state[k]] for k in CTX if latest[k]!=state[k]}
        emit('resumed',sequence=sequence,state=state,differences=diff)
        if not diff and sequence >= 32 and not injected:
            gdb.execute('set $rdx = 0x566e')
            injected=True
            emit('injected_witness',sequence=sequence,rip=reg('rip'),register='rdx',before=state['rdx'],after=reg('rdx'))
        return bool(diff)

Save('*'+hex(save),internal=True)
Restore('*'+hex(restore),internal=True)
Resume('*'+hex(resume),internal=True)
