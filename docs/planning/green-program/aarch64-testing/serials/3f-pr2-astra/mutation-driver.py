from pathlib import Path
import subprocess,hashlib,json,os,re
p=Path('kernel/src/task/scheduler.rs');original=p.read_text();root=Path.cwd();ev=root/'docs/planning/green-program/aarch64-testing/serials/3f-pr2-astra';env=os.environ.copy();env['TMPDIR']=str(root/'.tmp');env['BREENIX_GATE_TMP']=str(root/'.gate-tmp')
call='if self.retain_pinned_worker_on_source_queue(cpu, thread_id) {\n                    continue;\n                }'
guard='if self.retain_cpu_affine_thread(thread_id, current_cpu) {\n                    continue;\n                }'
stack='''        #[cfg(target_arch = "aarch64")]
        if let Some(slot) =
            crate::arch_impl::aarch64::constants::percpu_stack_slot_of(thread.context.sp)
        {
            if slot != pin.cpu {
                return false;
            }
        }
'''
offline='''        if source_cpu >= self.online_cpu_count() {
            return false;
        }
'''
legs=[('i-delete-call',original.replace(call,'',1),False),('ii-call-after-guard',original.replace(call+'\n                '+guard,guard+'\n                '+call,1),False),('iii-taking-queue',original.replace('retain_pinned_worker_on_source_queue(cpu, thread_id)','retain_pinned_worker_on_source_queue(current_cpu, thread_id)',1),False),('iv-drop-home-equality',original.replace('!pin.per_cpu_worker || pin.cpu != source_cpu','!pin.per_cpu_worker',1),False),('v-drop-stack-decline',original.replace(stack,'',1),False),('vi-count-stack-conflict',original.replace('if slot != pin.cpu {\n                return false;','if slot != pin.cpu {\n                PINNED_STACK_HOME_CONFLICT.fetch_add(1, Ordering::Relaxed);\n                return false;',1),False),('vii-drop-offline-decline',original.replace(offline,'',1),False)]
kick='''            #[cfg(target_arch = "aarch64")]
            self.send_resched_ipi_to_cpu(pin.cpu);
'''
start=original.index('    fn retain_cpu_affine_thread(')
for i in range(3):
 tail=original[start:];positions=[m.start() for m in re.finditer(re.escape(kick),tail)];at=start+positions[i];legs.append((f'viii-delete-kick-{i+1}',original[:at]+original[at:].replace(kick,'',1),False))
for i, match in enumerate(re.finditer(re.escape(kick), original[start:])):
 at = start + match.start()
 for arch in ['x86_64', 'riscv64']:
  changed = kick.replace('aarch64', arch)
  legs.append((f'ix-wrong-arch-{i+1}-{arch}', original[:at] + original[at:].replace(kick, changed, 1), False))
helper_at = original.index('    fn retain_pinned_worker_on_source_queue(')
legs.append(('x-separated-helpers', original[:helper_at] + '    fn unrelated() {}\n' + original[helper_at:], False))
legs += [('green-rename',original.replace('retain_pinned_worker_on_source_queue','restore_home_membership'),True),('green-trivia',original.replace('self.retain_pinned_worker_on_source_queue(', 'self /* receiver */ . retain_pinned_worker_on_source_queue /* call */ (').replace('cpu, thread_id) {','cpu /* source */, thread_id) {'),True),('green-sixth-site',original+'''
impl Scheduler {
    fn extra_rescue(&mut self, steal_cpu: usize, current_cpu: usize) {
        while let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
            if self.retain_pinned_worker_on_source_queue(steal_cpu, n) { continue; }
            if self.retain_cpu_affine_thread(n, current_cpu) { continue; }
            self.per_cpu_queues[current_cpu].push_back(n);
        }
    }
}
''',True)]
results=[]
try:
 for name,mutated,green in legs:
  assert mutated!=original,name
  p.write_text(mutated)
  r=subprocess.run(['bash','scripts/run-structure-tests.sh','loopback_pump_structure'],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  (ev/f'mutation-{name}.txt').write_text('revision='+subprocess.check_output(['git','rev-parse','HEAD'],text=True)+f'mutation={name}\noriginal_sha256={hashlib.sha256(original.encode()).hexdigest()}\nmutated_sha256={hashlib.sha256(mutated.encode()).hexdigest()}\nexit={r.returncode}\n'+r.stdout)
  p.write_text(original);assert p.read_text()==original
  expected=(r.returncode==0)==green
  results.append({'name':name,'result':('GREEN' if green else 'RED') if expected else 'UNEXPECTED','exit':r.returncode,'restored':True})
  print(results[-1],flush=True)
  if not expected: raise SystemExit('unexpected mutation result')
finally:
 p.write_text(original);(ev/'mutation-results.json').write_text(json.dumps(results,indent=2)+'\n')
