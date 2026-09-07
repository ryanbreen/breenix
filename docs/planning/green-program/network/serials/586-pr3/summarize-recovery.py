from pathlib import Path
import json,re
base=Path('docs/planning/green-program/network/serials/586-pr3')
rows=[]
for kind in ['extension-deleted','recovery','recovery-heavy','recovery-heavy-retry']:
 for cycle in sorted((base/kind).glob('cycle-*'),key=lambda p:int(p.name.split('-')[1])):
  serial=cycle/'serial.txt'
  if not serial.exists():continue
  text=serial.read_text()
  for line in re.findall(r'\[LOOPBACK_WAKE_BUDGET:[^\]]+\]',text):
   fields=dict(part.split('=',1) for part in line[1:-1].split(':')[1:] if '=' in part)
   if fields['test']!='when_idle':continue
   run=(base/kind/'run.txt').read_text()
   hogs=int(re.search(r'hogs=(\d+)',run).group(1))
   rows.append({'leg':kind,'hogs':hogs,'cycle':int(cycle.name.split('-')[1]),'serial':str(serial),'tick_ms':int(fields['elapsed_tick_ms']),'counter_ms':int(fields['elapsed_ctr_ms']),'ctx_delta':int(fields['ctx_delta']),'extensions':int(fields['extensions']),'verdict':fields['verdict'],'test_pass':'[TEST:network:loopback_recv_wake_when_idle:PASS]' in text})
(base/'starvation-attempts.json').write_text(json.dumps(rows,indent=2)+'\n')
print(json.dumps(rows[-2:],indent=2))
