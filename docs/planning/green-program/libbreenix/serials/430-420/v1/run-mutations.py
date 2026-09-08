from pathlib import Path
import os, subprocess, json
root=Path.cwd(); out=root/'docs/planning/green-program/libbreenix/serials/430-420/v1'
t='libs/libbreenix/src/ssh/transport.rs'; c='libs/libbreenix/src/ssh/channel.rs'; a='libs/libbreenix/src/ssh/auth.rs'; b='userspace/programs/src/bsshd.rs'; i='userspace/programs/src/init.rs'; cli='userspace/programs/src/bssh.rs'; g='docker/qemu/run-aarch64-boot-test-strict.sh'
mutations=[
('finish-api',t,'pub fn finish(&mut self, status: i32)','pub fn finish_disabled(&mut self, status: i32)'),
('close-sent',c,'if channel.close_sent {','if false {'),
('close-state',t,'ch.closed = true;','ch.closed = false;'),
('bitflip-negative',a,'wrong_identity.sign(data)','{ let mut signature = wrong_identity.sign(data)?; if let Some(last) = signature.last_mut() { *last ^= 0x01; } Ok(signature) }'),
('exec-finish',b,'session.finish(status)','session.close()'),
('shell-finish',b,'session.finish(shell_status)','session.close()'),
('wrong-signer',a,'wrong_identity.sign(data)','keys::sign_with_embedded_client_key(data)'),
('signature-verification',a,'keys::verify_rsa_signature(key_blob, signature, &signed_data)','true'),
('signature-algorithm',a,'signature_algo == Some(algo)','true'),
('refusal-exit',cli,'std::process::exit(77)','std::process::exit(0)'),
('oracle-status',i,'right == 0 && wrong == 77','right == 0 && wrong == 0'),
('oracle-marker',i,'[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]','[BSSH_PUBKEY_ORACLE:disabled]'),
('scorer-pass',g,'[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]','[BSSH_PUBKEY_ORACLE:disabled]'),
('scorer-fail',g,'[BSSH_PUBKEY_ORACLE:FAIL','[BSSH_PUBKEY_ORACLE:disabled'),
('recipient-check',t,'if recipient != ch.local_id {','if recipient != ch.local_id && false {'),
('recipient-remote',t,'if recipient != ch.local_id {','if recipient != ch.remote_id {'),
('close-length',t,'if pos != msg.len() {','if pos != msg.len() && false {'),
('truncated-default',t,'.ok_or(SshError::Protocol("bad channel close"))?;','.unwrap_or(0x01020304);'),
('early-closed',t,'let mut pos = 1;\n                let recipient =','self.channel.as_mut().unwrap().closed = true;\n                let mut pos = 1;\n                let recipient ='),
]
env=os.environ.copy();env.update(TMPDIR=str(root/'.tmp'),BREENIX_GATE_TMP=str(root/'.gate-tmp'))
results=[]
for name,path,old,new in mutations:
 p=root/path; original=p.read_bytes(); text=original.decode(); assert old in text, name
 try:
  p.write_text(text.replace(old,new))
  r=subprocess.run(['bash','scripts/run-structure-tests.sh','bssh_auth_close_structure'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  (out/f'mutation-{name}.txt').write_bytes(r.stdout+f'\nexit: {r.returncode}\n'.encode())
  results.append(dict(name=name,path=path,old=old,new=new,exit=r.returncode)); print(name,r.returncode,flush=True)
 finally: p.write_bytes(original)
(out/'mutations.json').write_text(json.dumps(results,indent=2)+'\n')
assert all(r['exit'] != 0 for r in results)
