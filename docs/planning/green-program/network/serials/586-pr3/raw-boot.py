import os,sys,subprocess,pathlib,shutil,time,hashlib,json,signal
root=pathlib.Path.cwd(); out=root/'docs/planning/green-program/network/serials/586-pr3'/sys.argv[1];out.mkdir(parents=True,exist_ok=True)
disk=root/'.gate-tmp'/('raw-'+sys.argv[1]+'.img');shutil.copyfile(root/'target/ext2-aarch64.img',disk)
kernel=root/'target/aarch64-breenix-kernel/release/kernel-aarch64'
args=['qemu-system-aarch64','-M','virt,gic-version=3','-cpu','cortex-a72','-m','512','-smp','4','-kernel',str(kernel),'-display','none','-no-reboot','-device','virtio-gpu-device','-device','virtio-keyboard-device','-device','virtio-tablet-device','-device','virtio-blk-device,drive=ext2','-drive',f'if=none,id=ext2,format=raw,file={disk}','-device','virtio-net-device,netdev=net0','-netdev','user,id=net0','-serial',f'file:{out}/serial.txt']
(out/'source.patch').write_bytes(subprocess.check_output(['git','diff','HEAD','--','kernel']))
meta={'revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'kernel_sha256':hashlib.sha256(kernel.read_bytes()).hexdigest(),'command':args,'kind':'direct recording boot; not a gate'}
with (out/'qemu.txt').open('w') as log:
 p=subprocess.Popen(args,stdout=log,stderr=log,start_new_session=True)
 try:
  p.wait(timeout=int(os.environ.get('RAW_BOOT_SECONDS','80')))
 except subprocess.TimeoutExpired:
  os.killpg(p.pid,signal.SIGTERM);p.wait(timeout=10)
 meta['qemu_exit']=p.returncode
(out/'run.json').write_text(json.dumps(meta,indent=2)+'\n');disk.unlink()
print(out)
