from pathlib import Path
import subprocess, os
root=Path.cwd(); output=root/'.tmp/813-c-mutation-replay'; output.mkdir(exist_ok=False); env=dict(os.environ,TMPDIR=str(root/'.tmp'))
mutations=[
('blocking-eagain','kernel/src/syscall/blocking_io.rs','wait_prepared(&crate::ipc::stdin::INPUT_READERS, outcome)?;', 'return Err(errno::EAGAIN);'),
('no-registration','kernel/src/ipc/stdin.rs','INPUT_READERS.prepare_to_wait_checked(', 'INPUT_READERS.prepare_to_wait('),
('delete-injection','userspace/programs/src/console_read_oracle.rs',"inject(fd, tid, b'8')?;",'/* injection removed */'),
('delete-wake','kernel/src/ipc/stdin.rs','INPUT_READERS.wake_up_deferred();','/* wake removed */'),
('no-copy','kernel/src/ipc/stdin.rs','buf[read] = self.buffer[self.read_pos];','/* copy removed */'),
('wrong-pty','kernel/src/syscall/blocking_io_oracle.rs','FdKind::Device(DeviceType::Tty)','FdKind::PtySlave(0)'),
('drop-open-flags','kernel/src/syscall/fs.rs','FdKind::Device(device.device_type),','FdKind::Device(crate::fs::devfs::DeviceType::Console),'),
('ignore-status-flags','kernel/src/syscall/handlers.rs','(fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0','false'),
('force-blocking','kernel/src/syscall/blocking_io.rs','if is_nonblocking {\n            return Err(errno::EAGAIN);','if !is_nonblocking {\n            return Err(errno::EAGAIN);'),
('no-pollin','kernel/src/ipc/poll.rs','(events & events::POLLIN) != 0 && crate::ipc::stdin::has_data()','false'),
('always-pollin','kernel/src/ipc/poll.rs','(events & events::POLLIN) != 0 && crate::ipc::stdin::has_data()','(events & events::POLLIN) != 0'),
('wrong-queue','kernel/src/ipc/poll.rs','crate::ipc::stdin::has_data()','independent_tty_queue_has_data()'),
('omit-signal','kernel/src/syscall/blocking_io.rs','crate::syscall::check_signals_for_eintr().is_some()','false'),
('leak-waiter','kernel/src/syscall/blocking_io.rs','queue.take_waiter(tid);','/* cleanup removed */'),
('destructive-query','kernel/src/syscall/blocking_io_oracle.rs','let (queued, occupancy) = crate::ipc::stdin::input_witness(witness.tid);','let _ = crate::ipc::stdin::read_bytes(&mut [0; 1]);\n    let (queued, occupancy) = crate::ipc::stdin::input_witness(witness.tid);'),
('null-blocks','kernel/src/fs/devfs/mod.rs','// /dev/null always returns EOF (0 bytes read)\n            Ok(0)','crate::ipc::stdin::read_bytes(buf)'),
('zero-routes-stdin','kernel/src/fs/devfs/mod.rs','Ok(buf.len())','crate::ipc::stdin::read_bytes(buf)'),
('zero-count','kernel/src/syscall/handlers.rs','if buf_ptr == 0 || count == 0','if buf_ptr == 0'),
('wrong-partial-count','kernel/src/ipc/stdin.rs','let to_read = buf.len().min(self.len);','let to_read = buf.len();'),
]
summary=[]
for name,file,old,new in mutations:
 p=root/file; original=p.read_text(); assert old in original, name
 try:
  p.write_text(original.replace(old,new))
  suite='blocking_fd_eagain_structure' if name=='force-blocking' else 'console_read_structure'
  r=subprocess.run(['bash','scripts/run-structure-tests.sh',suite],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  (output/f'mutation-{name}.txt').write_text('revision='+subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()+' + PR C working-tree single mutation\n'+file+'\n'+r.stdout+f'\nexit={r.returncode}\n')
  summary.append(f'{name}: exit {r.returncode}')
  if r.returncode==0: raise RuntimeError('SURVIVED '+name)
 finally: p.write_text(original)
(output/'mutations.txt').write_text('\n'.join(summary)+'\nAll mutations restored by finally; structural rejection only, not runtime mutation evidence.\n')
print('\n'.join(summary))
