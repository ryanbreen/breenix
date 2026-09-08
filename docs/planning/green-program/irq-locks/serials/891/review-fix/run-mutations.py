from pathlib import Path
import os, subprocess
root=Path.cwd()
env=dict(os.environ,TMPDIR=str(root/'.tmp'),BREENIX_GATE_TMP=str(root/'.gate-tmp'),PATH='/opt/homebrew/bin:'+os.environ['PATH'])
out=root/'docs/planning/green-program/irq-locks/serials/891/review-fix'
out.mkdir(exist_ok=True)
cases=[
('v1-score-only','docker/qemu/run-aarch64-boot-test-strict.sh','exit "$SCORE_STATUS"','exit 1','strict_gate_preserves_starvation_status'),
('v1-frozen-status','docker/qemu/run-aarch64-boot-test-strict.sh','        SCORE_STATUS=$?\n    fi','        SCORE_STATUS=1\n    fi','strict_gate_preserves_starvation_status'),
('v1-import','docker/qemu/run-aarch64-boot-test-strict.sh','aarch64 strict INCONCLUSIVE 2','aarch64 strict FAIL 1','strict_gate_preserves_starvation_status'),
('v1-aggregate','docker/qemu/run-aarch64-boot-test-strict.sh','BOOT_STATUS=$?','BOOT_STATUS=1','strict_gate_preserves_starvation_status'),
('v2-backstop','scripts/score-softirq-deferral.py','int(ns) < 15_000_000_000','int(ns) < 0','scorer_rejects_forged_ok_and_reports_starvation_separately'),
('v3-format','kernel/src/task/mod.rs','register_softirq_handler, SoftirqHandler, SoftirqType,','register_softirq_handler, SoftirqHandler,\n    SoftirqType,','softirq_reexport_is_rustfmt_clean'),
('omit-wake','kernel/src/task/softirqd.rs','        wakeup_ksoftirqd();','        // mutation: omit wake','limit_wakes_daemon_and_counter_is_at_callback'),
('boot-publish','kernel/src/task/softirqd.rs','cpu == 0 && crate::per_cpu_aarch64::preempt_count() != 0','false','boot_cpu_daemon_is_published_only_after_boot_preemption_pin_is_released'),
]
for name,file,old,new,test in cases:
 p=root/file; original=p.read_bytes(); text=original.decode(); assert old in text,(name,old)
 try:
  p.write_text(text.replace(old,new,1))
  r=subprocess.run(['bash','scripts/run-structure-tests.sh','softirq_deferral_structure',test],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  (out/(name+'-red.txt')).write_bytes(r.stdout+f'\nexit={r.returncode}\n'.encode())
  assert r.returncode==101,(name,r.stdout.decode())
 finally:
  p.write_bytes(original)
 assert p.read_bytes()==original
 print(name,'exit=101; restored',flush=True)
r=subprocess.run(['bash','scripts/run-structure-tests.sh','softirq_deferral_structure'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
(out/'restored-green.txt').write_bytes(r.stdout)
assert r.returncode==0,r.stdout.decode()
