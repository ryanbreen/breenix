#!/usr/bin/env python3
# Build kernels and disk images first; this runner retains its fixture, serials and failed verdicts.
import os, pathlib, shutil, signal, subprocess, sys, time, json, resource
root=pathlib.Path(__file__).resolve().parents[1]
arch,leg=sys.argv[1:3]
if arch not in ('x86', 'arm') or leg not in ('accept', 'readonly'):
    raise SystemExit('usage: ext2-acceptance.sh x86|arm accept|readonly OUTPUT')
if arch == "x86":
    policy = subprocess.run(["bash", str(root / "scripts/x86-qemu-policy.sh")])
    if policy.returncode:
        raise SystemExit(policy.returncode)
out=pathlib.Path(sys.argv[3]).resolve()
if (out/'qemu.pid').exists(): raise RuntimeError('retain the previous boot folder')
out.mkdir(parents=True, exist_ok=True)
serial=out/'serial_kernel.txt'; user=out/'serial_user.txt'; trace=out/'block-writes.txt'
readonly=leg=='readonly'
image=root/'target'/('ext2.img' if arch=='x86' else 'ext2-aarch64.img')
shutil.copyfile(image,out/'root.img')
shutil.copyfile(image,out/'home.img')
marker=out/'readonly-marker.txt'; marker.write_text('Read-only ext2 acceptance fixture\n')
debugfs=shutil.which('debugfs') or '/opt/homebrew/opt/e2fsprogs/sbin/debugfs'
with (out/'fixture.log').open('w') as f:
    subprocess.run(['taskpolicy','-b',debugfs,'-w','-R',f'write {marker} /.breenix-readonly-fixture',str(out/'home.img')],stdout=f,stderr=subprocess.STDOUT,check=True)
check=subprocess.check_output([debugfs,'-R','stat /.breenix-readonly-fixture',str(out/'home.img')],stderr=subprocess.STDOUT,text=True)
if 'Inode:' not in check: raise RuntimeError('read-only fixture marker missing')
cmd=['qemu-system-'+('x86_64' if arch=='x86' else 'aarch64')]
if arch=='x86':
    for source,name in [('code.fd','OVMF_CODE.fd'),('vars.fd','OVMF_VARS.fd')]:
        override='BREENIX_OVMF_CODE_PATH' if source=='code.fd' else 'BREENIX_OVMF_VARS_PATH'
        shutil.copyfile(os.environ.get(override, root/'target/ovmf/x64'/source),out/name)
    launcher_env=dict(os.environ, BREENIX_PRINT_UEFI_IMAGE='1')
    launcher_env.setdefault('BREENIX_OVMF_CODE_PATH', str(root/'target/ovmf/x64/code.fd'))
    launcher_env.setdefault('BREENIX_OVMF_VARS_PATH', str(root/'target/ovmf/x64/vars.fd'))
    output=subprocess.check_output([str(root/'target/release/qemu-uefi')], env=launcher_env, text=True)
    uefi=pathlib.Path(next(line.removeprefix('UEFI_IMAGE=') for line in output.splitlines() if line.startswith('UEFI_IMAGE=')))

    shutil.copyfile(uefi,out/'breenix-uefi.img'); uefi=out/'breenix-uefi.img'
    cmd+=['-drive',f'if=pflash,format=raw,readonly=on,file={out}/OVMF_CODE.fd','-drive',f'if=pflash,format=raw,file={out}/OVMF_VARS.fd']
    for name,path,ro in [('hd',uefi,True),('testdisk',root/'target/test_binaries.img',True),('ext2disk',out/'root.img',readonly),('homedisk',out/'home.img',True)]:
        cmd+=['-drive',f'if=none,id={name},format=raw,'+('readonly=on,' if ro else 'snapshot=on,')+f'file={path}', '-device',f'virtio-blk-pci,drive={name},disable-modern=on,disable-legacy=off'+(',bootindex=0' if name=='hd' else '')]
    cmd+=['-machine','pc,accel=tcg','-cpu','qemu64','-smp','4','-m','512','-rtc','base=utc','-device','isa-debug-exit,iobase=0xf4,iosize=0x04','-serial',f'file:{user}','-serial',f'file:{serial}','-netdev','user,id=net0','-device','e1000,netdev=net0,mac=52:54:00:12:34:56']
    cmd+=['-no-shutdown']
    profile='tcg-qemu64'; cpus='4'; deadline=900
else:
    kernel=root/'target/aarch64-breenix-kernel/release/kernel-aarch64'
    shutil.copyfile(kernel,out/'kernel-aarch64')
    cmd+=['-M','virt,gic-version=3,accel=tcg','-cpu','cortex-a72','-m','512','-smp','4','-kernel',str(out/'kernel-aarch64'),'-device','virtio-gpu-device','-device','virtio-keyboard-device','-device','virtio-tablet-device']
    # Match the driver's ascending MMIO scan: QEMU allocates these buses downward.
    for name,ro in [('home',True),('root',readonly)]:
        cmd+=['-device',f'virtio-blk-device,drive={name}','-drive',f'if=none,id={name},format=raw,'+('readonly=on,' if ro else '')+f'file={out}/{name}.img']
    cmd+=['-device','virtio-net-device,netdev=net0','-netdev','user,id=net0','-serial',f'file:{serial}']
    profile='tcg-cortex-a72'; cpus='4'; deadline=90
cmd+=['-display','cocoa','-no-reboot','-trace',f'enable=virtio_blk_handle_write,file={trace}']
(out/'command.json').write_text(json.dumps(cmd,indent=2))
serial.touch(); user.touch()
started=int(time.time()); identifier=time.strftime('%Y%m%dT%H%M%SZ',time.gmtime())+'-ext2-'+arch+'-'+leg
(out/'id').write_text(identifier)
commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
dirty=subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=root,text=True).count('\n')
env=dict(os.environ, BREENIX_VIGIL_ARCH=('x86_64' if arch=='x86' else 'aarch64'), BREENIX_VIGIL_CPUS=cpus)
recorder=['bash',str(root/'scripts/vigil-record.sh')]
record_args=['qemu','tests','',str(serial),str(user) if arch=='x86' else '',profile,identifier,commit]
with (out/'vigil-start.log').open('w') as f:
    subprocess.run(recorder+['start']+record_args,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
p=None; reason=''; status=1; qemu_cpu=None; qemu_wall=None
try:
    log=(out/'qemu.log').open('w')
    usage_before=resource.getrusage(resource.RUSAGE_CHILDREN)
    qemu_start=time.monotonic()
    p=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT)
    (out/'qemu.pid').write_text(str(p.pid))
    def interrupted(sig,frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM,interrupted); signal.signal(signal.SIGINT,interrupted)
    end=time.monotonic()+deadline
    offsets={serial:0,user:0}; tails={serial:'',user:''}; seen=set()
    needles=['KERNEL PANIC:','KERNEL PANIC!','Os(EROFS)','FS_WRITE_TEST_FAILED','BLOCK_ALLOC_TEST_FAILED','USERSPACE TEST REPORT DONE','READONLY_EXT2: PASS']
    while time.monotonic()<end and p.poll() is None:
        for path in [serial,user]:
            if not path.exists(): continue
            with path.open('rb') as f:
                f.seek(offsets[path]); data=f.read(); offsets[path]=f.tell()
            chunk=tails[path]+data.decode(errors='replace')
            seen.update(needle for needle in needles if needle in chunk)
            tails[path]=chunk[-4096:]
        if 'KERNEL PANIC:' in seen or 'KERNEL PANIC!' in seen:
            reason='kernel panic'; break
        if readonly and all(x in seen for x in ['Os(EROFS)','FS_WRITE_TEST_FAILED','BLOCK_ALLOC_TEST_FAILED','READONLY_EXT2: PASS']):
            status=1; reason='EROFS cases collected; original mutation tests remain failed'; break
        if arch=='x86' and not readonly and 'USERSPACE TEST REPORT DONE' in seen and time.monotonic()<=end:
            status=0; reason='userspace report done'; break
        time.sleep(.25)
    else: reason='deadline or QEMU exited'
except KeyboardInterrupt:
    reason='interrupted'; status=130
finally:
    if p is not None and p.poll() is None:
        p.terminate()
        try: p.wait(timeout=5)
        except subprocess.TimeoutExpired: p.kill(); p.wait()
    if p is not None:
        usage_after=resource.getrusage(resource.RUSAGE_CHILDREN)
        qemu_cpu=(usage_after.ru_utime+usage_after.ru_stime)-(usage_before.ru_utime+usage_before.ru_stime)
        qemu_wall=time.monotonic()-qemu_start
    text='\n'.join(path.read_text(errors='replace') if path.exists() else '' for path in [serial,user])
    if not readonly:
        score_env=dict(os.environ)
        if arch=='arm':
            score_env.update(BREENIX_STRICT_SCORE_ONLY=str(serial),BREENIX_STRICT_SCORE_KERNEL=str(out/'kernel-aarch64'))
            scoring=['bash',str(root/'docker/qemu/run-aarch64-boot-test-strict.sh')]
        else:
            score_env['EXPECTED_EXITS']='105'
            scoring=['bash',str(root/'scripts/x86-gate-verdict.sh'),str(serial),str(user)]
        with (out/'score.log').open('w') as f:
            status=subprocess.run(scoring,cwd=root,env=score_env,stdout=f,stderr=subprocess.STDOUT).returncode
        required=(['USERSPACE TEST REPORT DONE','READONLY_EXT2: PASS','LOOKUP_DURING_SHRINK: uid=1001','BLOCK_ALLOC_TEST_PASSED','FS_WRITE_TEST_PASSED','FSYNC_DURING_SHRINK: PASS','TRUNCATE_10MIB:'])
        missing=[marker for marker in required if marker not in text]
        if missing:
            status=1
            with (out/'score.log').open('a') as f: f.write('Required ext2 evidence missing: '+str(missing)+'\n')
        if '[BOOT_TESTS:FAIL' in text or 'LOOKUP_DURING_SHRINK: NOT_EXERCISED' in text:
            status=1
        if not trace.exists() or not trace.read_text().strip():
            status=1
            with (out/'score.log').open('a') as f: f.write('Writable-root trace positive control missing\n')
        reason='scored captured boot'
    else:
        writes=trace.read_text() if trace.exists() else 'trace missing'
        if writes.strip(): status=1; reason='read-only backend write or trace missing'
    ended=int(time.time())
    (out/'result.json').write_text(json.dumps(dict(status=status,reason=reason,started=started,ended=ended,pid=None if p is None else p.pid,qemu_cpu_seconds=qemu_cpu,qemu_wall_seconds=qemu_wall),indent=2))
    with (out/'vigil-record.log').open('w') as f:
        subprocess.run(recorder+['record']+record_args+[str(started),str(ended),str(status)],env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
print(json.dumps(dict(id=identifier,status=status,reason=reason)))
sys.exit(status)
