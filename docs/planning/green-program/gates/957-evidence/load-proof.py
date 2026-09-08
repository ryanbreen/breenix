import os,subprocess,time,pathlib
root=pathlib.Path.cwd(); out=root/'.tmp/proof';out.mkdir(exist_ok=True)
os.environ.update(TMPDIR=str(root/'.tmp'),BREENIX_GATE_TMP=str(root/'.gate-tmp'))
hogs=[subprocess.Popen(['yes'],stdout=subprocess.DEVNULL) for _ in range(8)]
try:
 with (out/'load-results.txt').open('w') as result:
  result.write(subprocess.check_output(['git','rev-parse','HEAD'],text=True));result.flush()
  for i in range(1,22):
   name=f'drain-{i:02}' if i<=20 else 'all-suites'
   cmd=['bash','scripts/run-structure-tests.sh']+(['gate_capture_drain_structure'] if i<=20 else [])
   start=time.monotonic()
   with (out/f'{name}.log').open('w') as log:r=subprocess.run(cmd,stdout=log,stderr=subprocess.STDOUT)
   result.write(f'{name} {time.monotonic()-start:.3f}s exit={r.returncode}\n');result.flush()
finally:
 for p in hogs:p.terminate()
 for p in hogs:p.wait()
