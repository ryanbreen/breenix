import pathlib, subprocess, re
root=pathlib.Path('/root/breenix-927a')
out=pathlib.Path('/root/breenix-927a-tmp')
elf=next(root.glob('target/x86_64-unknown-none/release/deps/artifact/kernel-*/bin/kernel-*'))
serial=''.join(p.read_text(errors='replace') for p in (out/'repro').glob('serial_*.txt'))
base=int(re.search(r'virtual_address_offset:\s*(0x[0-9a-fA-F]+)',serial)[1],16)
nm=subprocess.check_output(['nm','-C',str(elf)],text=True)
def addr(name):
 matches=[int(l.split()[0],16) for l in nm.splitlines() if l.split(' ',2)[-1]==name]
 assert len(matches)==1,(name,matches)
 return base+matches[0]
pair=addr('kernel::tracing::providers::teardown::fork_exit_defer_reclaim_pairing_test')
fork=addr('kernel::process::manager::ProcessManager::fork_process_with_page_table')
retire=addr('PT_ROOTS_RETIRED')+64
entry=addr('TEARDOWN_ENTRY_EXIT')+64
frames=addr('kernel::memory::frame_allocator::NEXT_FREE_FRAME')
cmd=f'''set pagination off
set confirm off
set language c
file {elf}
add-symbol-file {elf} -o {base:#x}
target remote 127.0.0.1:1927
hbreak *{pair:#x}
continue
printf "PAIRING ENTRY\\n"
x/gd {retire:#x}
x/gd {entry:#x}
x/gd {frames:#x}
delete 1
set $fork_hits = 0
hbreak *{fork:#x}
continue
set $fork_hits = $fork_hits + 1
printf "PAIRING FORK HIT=%d; zero-based iteration=%d\\n", $fork_hits, $fork_hits-1
set $result = $rdi
set $return = *(unsigned long long*)$rsp
info registers rdi rsi rdx rsp rip
bt 5
x/gd {retire:#x}
x/gd {entry:#x}
x/gd {frames:#x}
tbreak *$return
continue
printf "FORK RETURN RESULT (Rust Result discriminant, string pointer, string length)\\n"
x/3gx $result
x/s *(unsigned long long*)($result+8)
x/gd {retire:#x}
x/gd {entry:#x}
x/gd {frames:#x}
detach
quit
'''
(out/'gdb-commands.txt').write_text(cmd)
print('REVISION='+subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),flush=True)
print(f'BASE={base:#x} ELF={elf}',flush=True)
subprocess.run(['gdb','--nx','-q','--batch','-x',str(out/'gdb-commands.txt')],check=True)
