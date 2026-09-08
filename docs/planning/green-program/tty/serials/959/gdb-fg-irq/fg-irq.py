import gdb

gdb.execute('set pagination off')
gdb.execute('set confirm off')
gdb.execute('target remote localhost:19599')
b=gdb.Breakpoint('*0xffff00004056272c')
gdb.execute('continue')
print('FG_AFTER_START_TIMESTAMP')
gdb.execute('info registers pc cpsr')
regs=gdb.execute('info all-registers',to_string=True)
print('\n'.join(line for line in regs.splitlines() if any(x in line.lower() for x in ['cntv','cntp','cntfrq','cpsr'])))
gdb.execute('detach')
