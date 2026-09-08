import gdb

gdb.execute('set pagination off')
gdb.execute('set confirm off')
gdb.execute('target remote localhost:19599')
b=gdb.Breakpoint('*0xffff00004056272c')
b.condition = '$x1 == 3'
gdb.execute('continue')
print('FG_AFTER_START_TIMESTAMP')
gdb.execute('info registers pc cpsr')
regs=gdb.execute('info all-registers',to_string=True)
print('\n'.join(line for line in regs.splitlines() if any(x in line.lower() for x in ['cntv','cntp','cntfrq','cpsr'])))
b.delete()
irq = gdb.Breakpoint('*0xffff0000404ab268')
irq.thread = gdb.selected_thread().num
# Adversarial timer delivery in the measured window; no kernel code is changed.
gdb.execute('set $CNTV_CVAL_EL0 = 0')
gdb.execute('continue')
frame = int(gdb.parse_and_eval('$x0')) & ((1 << 64) - 1)
raw = gdb.selected_inferior().read_memory(frame + 248, 16)
elr = int.from_bytes(raw[:8], 'little')
spsr = int.from_bytes(raw[8:], 'little')
print('FORCED_TIMER_IN_MEASURED_FG elr=%#x saved_spsr=%#x' % (elr, spsr))
gdb.execute('x/2i %#x' % elr)
irq.delete()
gdb.execute('detach')
