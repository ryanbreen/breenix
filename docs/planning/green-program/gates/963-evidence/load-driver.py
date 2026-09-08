import os, pathlib, subprocess, time, json
root=pathlib.Path.cwd(); ev=root/'docs/planning/green-program/gates/963-evidence'
os.environ.update(TMPDIR=str(root/'.tmp'), BREENIX_GATE_TMP=str(root/'.gate-tmp'))
rev=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
rows=[]
def run(label):
 t=time.monotonic(); r=subprocess.run(['timeout','300','bash','scripts/run-structure-tests.sh','gate_qmp_backstop_structure'],capture_output=True,text=True); elapsed=time.monotonic()-t
 (ev/(label+'.log')).write_text(f'revision={rev}\ncommand=timeout 300 bash scripts/run-structure-tests.sh gate_qmp_backstop_structure\n'+r.stdout+r.stderr+f'exit={r.returncode} wall={elapsed:.3f}s\n')
 rows.append(dict(run=label,exit=r.returncode,seconds=round(elapsed,3)))
 (ev/'load-results.json').write_text(json.dumps(rows,indent=2)+'\n')
 print(rows[-1],flush=True)
 assert r.returncode==0
run('normal')
hogs=[subprocess.Popen(['python3','-c','while True: pass']) for _ in range(8)]
(ev/'hog-pids.log').write_text(f'revision={rev}\n'+str([p.pid for p in hogs])+'\n')
try:
 for i in range(1,21):
  assert all(p.poll() is None for p in hogs)
  run(f'load-{i:02}')
finally:
 for p in hogs: p.terminate()
 for p in hogs: p.wait()
 print('Tracked eight hogs reaped',flush=True)
