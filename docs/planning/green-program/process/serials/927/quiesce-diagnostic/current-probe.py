import pathlib,subprocess
root=pathlib.Path('/root/breenix-927a');out=pathlib.Path('/root/breenix-927a-tmp');elf=next(root.glob('target/x86_64-unknown-none/release/deps/artifact/kernel-*/bin/kernel-*'))
nm=subprocess.check_output(['nm','-C',str(elf)],text=True)
def addr(name):
 matches=[int(l.split()[0],16) for l in nm.splitlines() if l.split(' ',2)[-1]==name]
 assert len(matches)==1,(name,matches)
 return 0x10000000000+matches[0]
pair=addr('kernel::tracing::providers::teardown::fork_exit_defer_reclaim_pairing_test')
drain=addr('kernel::task::process_task::boot_reclaim_deferred_process_resources')
cmd=f'''set pagination off
set confirm off
set language c
file {elf}
add-symbol-file {elf} -o 0x10000000000
target remote 127.0.0.1:1927
hbreak *{pair:#x}
continue
delete 1
hbreak *{drain:#x}
continue
printf "FIRST COHORT DRAIN\\n"
bt 5
'''
for name in ['PT_ROOTS_RETIRED','TEARDOWN_ENTRY_EXIT','PT_RETIRE_BUDGET_REQUEUED','PT_RETIRE_FRAMES_LOST']:
 cmd+=f'printf "{name}="\nx/gd {addr(name)+64:#x}\n'
for name in ['kernel::task::process_task::BOOT_RECLAIM_TEST_OWNER','kernel::task::process_task::BOOT_RECLAIM_PASS_START','kernel::task::process_task::BOOT_RECLAIM_PASS_SELECTIONS']:
 cmd+=f'printf "{name}="\nx/gd {addr(name):#x}\n'
cmd+='delete 2\ndetach\nquit\n'
(out/'current-probe.gdb').write_text(cmd)
print('REVISION='+subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),flush=True)
subprocess.run(['gdb','--nx','-q','--batch','-x',str(out/'current-probe.gdb')],check=True)
