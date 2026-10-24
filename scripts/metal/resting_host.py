"""Boot-host preemption worker: private keys never enter the metal container."""
import argparse
import fcntl
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import time


def preemptible(run):
    return (run.get('resting') is True and run.get('session')
            and not run.get('run') and not run.get('upgradeParent'))


def preempt(waiter, only_persistent=False):
    runner = runpy.run_path(str(Path(__file__).with_name('metal-run')))
    env = runner['host_env']()
    def read(path):
        result = runner['in_metal'](env, 'cat', runner['STATE'] + '/' + path, capture_output=True, text=True)
        return json.loads(result.stdout) if result.returncode == 0 else {}
    # Ordinary tests and a first install need neither an operator key nor a
    # previous pin. Consult private authentication only when actually preempting.
    while True:
        claim = read('boots/' + waiter + '/claim.json')
        if claim.get('status') in ('claimed', 'busy', 'quarantined', 'not-ready'):
            return
        if runner['in_metal'](env, 'test', '-e', runner['STATE'] + '/boots/' + waiter + '/ended').returncode == 0:
            return
        lease = read('lease.json')
        if lease.get('kind') == 'persistent' and lease.get('id') != waiter:
            holder = read('boots/' + lease['id'] + '/run.json')
            if preemptible(holder):
                break
            if only_persistent:
                return
        elif only_persistent:
            return
        time.sleep(2)
    api = runpy.run_path(str(Path(__file__).with_name('server-suite')))
    lab_type = runpy.run_path(str(Path(__file__).with_name('server_resting.py')))['Lab']
    with tempfile.TemporaryDirectory(prefix='metal-preempt-') as scratch:
        lab = lab_type(api, argparse.Namespace(boot=waiter), Path(scratch))
        state = Path.home() / '.local/state/breenix-metal'
        state.mkdir(parents=True, exist_ok=True, mode=0o700)
        deadline = time.monotonic() + 180
        while not lab.ended(waiter):
            if time.monotonic() >= deadline:
                raise ValueError('resting preemption exceeded 180 seconds; cancel request and inspect holder')
            claim = json.loads(lab.read(waiter, 'claim.json') or '{}')
            if claim.get('status') == 'claimed':
                return
            lease = lab.lease()
            holder = lease.get('id')
            if only_persistent and lease.get('kind') != 'persistent':
                return
            if holder and holder != waiter and lease.get('kind') == 'persistent':
                run = json.loads(lab.read(holder, 'run.json') or '{}')
                # A verification boot or command batch finishes on its own. It
                # must retain the lease through automatic upgrade verification.
                if preemptible(run):
                    with (state / ('preempt-' + holder + '.lock')).open('w') as handle:
                        try:
                            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        except BlockingIOError:
                            time.sleep(2)
                            continue
                        if json.loads(lab.read(holder, 'preemption.json') or '{}').get('complete'):
                            return
                        if lab.lease().get('id') != holder or not any(q.get('id') == waiter for q in lab.lease().get('queue', [])):
                            continue
                        layout = lab.remote('python3', '-c', 'import sys,json;sys.path.insert(0,"/opt/breenix-metal");'
                            'import metal_persist;print(json.dumps(metal_persist.read_disk()["layout"]))', check=True)
                        system = json.loads(layout.stdout)['partitions']['system']['guid']
                        key_id = hashlib.sha256((system + ':' + lab.site['SERVER_SSH_HOST']).encode()).hexdigest()
                        cache = json.loads((state / ('pinned-hostkey-' + key_id + '.json')).read_text())
                        lab.probe.known.write_text(cache['known'])
                        result = lab.probe.ssh('id -u')
                        if result.returncode:
                            # POST and a transient service restart need no
                            # recovery. The queued request keeps its own bound.
                            time.sleep(2)
                            continue
                        lab.probe.require(result, lab.site['OPERATOR_UID'])
                        if lab.lease().get('id') != holder:
                            continue
                        lab.args.boot = holder
                        requested = time.time()
                        lab.power('poweroff')
                        lab.wait(lambda: lab.ended(holder), 120, 'persistent shutdown did not finish')
                        lab.save('preemption.json', dict(waiter=waiter, requestedAt=requested,
                            authenticated=True, strictHostKey=True, complete=True))
                        return
            time.sleep(2)
