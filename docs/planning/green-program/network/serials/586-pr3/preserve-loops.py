from pathlib import Path
import shutil,re
root=Path.cwd();base=root/'docs/planning/green-program/network/serials/586-pr3'
for loop,dest in [('extension-deleted-loop','extension-deleted'),('recovery-loop','recovery'),('recovery-heavy','recovery-heavy'),('recovery-heavy-retry','recovery-heavy-retry')]:
 src=root/'.gate-tmp'/loop
 if not (src/'run.txt').exists():continue
 out=base/dest;out.mkdir(exist_ok=True)
 run=(src/'run.txt').read_text()
 for n in re.findall(r'STARVED_LOOP_FACTS:[^\n]*:cycle=(\d+):',run):
  cycle=src/f'cycle-{n}';target=out/cycle.name;target.mkdir(exist_ok=True)
  raw=(cycle/'gate.txt').read_bytes()
  (target/'gate-raw.txt').write_bytes(raw)
  revision=re.search(r'revision=([0-9a-f]+)',run).group(1)
  header=f'archived_driver_provenance: revision={revision} cycle={n}; attributed from enclosing run.txt; original gate output follows\n'
  (target/'gate.txt').write_bytes(header.encode()+raw)
  for file in ['serial.txt','gate_boot_facts.txt']:
   f=cycle/'breenix_aarch64_strict_1'/file
   if f.exists():shutil.copyfile(f,target/file)
 shutil.copyfile(src/'run.txt',out/'run.txt')
