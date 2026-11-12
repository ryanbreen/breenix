"""Run the ARM terminal replay choice through the real stalled-owner model."""
from pathlib import Path
import os,re,subprocess,tempfile,unittest
from serial_records_test import function
ROOT=Path(__file__).resolve().parents[1]
class TerminalReplay(unittest.TestCase):
 def test_complete_host_request_is_replayed_after_an_owner_stall(self):
  source=(ROOT/'kernel/src/serial_line.rs').read_text()
  if 'fn terminal_kind(' in source:policy=function(source,'fn terminal_kind(')
  else:
   sender=function(source,'fn send_queued_owned(')
   mode=re.search(r'Writer::Tty, sent, (Kind::[A-Za-z]+),',sender)[1]
   policy='fn terminal_kind(_: &[u8])->Kind {'+mode+'}'
  harness=(ROOT/'tests/uart_owner.rs').read_text()
  for name in ['stage_lines','uart_owner']:
   harness=harness.replace('../kernel/src/'+name+'.rs',str(ROOT/'kernel/src'/ (name+'.rs')))
  harness+='\n'+policy+r'''
#[test]fn terminal_request_survives_the_captured_stall(){
 let _one=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
 let request=b"SUITE window-manager HOSTPOINTER stacking/exit-restacks seq=336 x=2520 y=1500 width=2560 height=1600 buttons=0 wheel=0\n";
 let lines=race((request,terminal_kind(request)),12,(b"[syscall] exit(0)\n",Kind::Line),b"[syscall] exit(0)\n".len()+2,0);
 assert!(lines.iter().any(|l| l.as_bytes()==&request[..request.len()-1]),"host request was split around the diagnostic: {lines:?}");
}
'''
  with tempfile.TemporaryDirectory(prefix='terminal-replay-')as directory:
   path=Path(directory);(path/'check.rs').write_text(harness)
   subprocess.run([os.environ.get('RUSTC','rustc'),'--edition=2021','--test','-Dwarnings',str(path/'check.rs'),'-o',str(path/'check')],check=True)
   subprocess.run([str(path/'check'),'--exact','terminal_request_survives_the_captured_stall'],check=True)
if __name__=='__main__':unittest.main()
