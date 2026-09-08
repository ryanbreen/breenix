#!/usr/bin/env python3
"""Record raw SSH output plus revision, command, exit status and timing sidecars."""
import subprocess,time,json,shlex,os,argparse
from pathlib import Path
parser=argparse.ArgumentParser(description="Capture bounded OpenSSH acceptance probes for bsshd.")
parser.add_argument("--port",required=True)
parser.add_argument("--identity",type=Path,required=True)
parser.add_argument("--known-hosts",type=Path,required=True)
parser.add_argument("--output",type=Path,required=True)
opts=parser.parse_args()
out=opts.output
out.mkdir(parents=True,exist_ok=True)
base=['timeout','15','ssh','-vv','-p',opts.port,'-o','UserKnownHostsFile='+str(opts.known_hosts.resolve()),'-o','GlobalKnownHostsFile=/dev/null','-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','PubkeyAcceptedAlgorithms=rsa-sha2-256','-i',str(opts.identity.resolve())]
tests=[('pin','accept-new',['true'],None,0),('true','yes',['true'],None,0),('echo','yes',['echo hi'],None,0),('false','yes',['false'],None,1),('shell','yes',['-tt'],b'echo interactive-ok\nexit 7\n',7),('shell-no-pty','yes',['-T'],b'echo pipe-ok\nexit 3\n',3)]
results=[]
for name,strict,args,stdin,want in tests:
 cmd=base+['-o','StrictHostKeyChecking='+strict]
 if args[0].startswith('-'): cmd+=args+['root@127.0.0.1']
 else: cmd+=['root@127.0.0.1']+args
 start=time.monotonic()
 if name == 'shell':
  proc=subprocess.Popen(cmd,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  data=b''
  sent=False
  while True:
   chunk=os.read(proc.stdout.fileno(),4096)
   if not chunk: break
   data+=chunk
   if not sent and b'bsh />' in data:
    proc.stdin.write(stdin);proc.stdin.flush();sent=True
  proc.wait()
  p=subprocess.CompletedProcess(cmd,proc.returncode,data)
 else:
  p=subprocess.run(cmd,input=stdin,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 elapsed=time.monotonic()-start
 (out/(name+'.txt')).write_bytes(p.stdout)
 record={'name':name,'revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'dirty':bool(subprocess.check_output(['git','diff','--name-only'])),'command':shlex.join(cmd),'stdin':stdin.decode() if stdin else None,'exit':p.returncode,'expected':want,'seconds':round(elapsed,3)}
 (out/(name+'.json')).write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps(record),flush=True)
 record['proof_ok']=(p.returncode==want and
     b'using "publickey"' in p.stdout and
     b'rtype exit-status reply 0' in p.stdout and
     b'rcvd eof' in p.stdout and b'rcvd close' in p.stdout and
     b'send close for remote id' in p.stdout and
     (name!='echo' or b'hi' in p.stdout.splitlines()) and
     (name!='shell' or b'interactive-ok' in p.stdout.splitlines()) and
     (name!='shell-no-pty' or b'pipe-ok' in p.stdout.splitlines()))
 (out/(name+'.json')).write_text(json.dumps(record,indent=2)+'\n')
 results.append(record)
raise SystemExit(any(not r['proof_ok'] for r in results))
