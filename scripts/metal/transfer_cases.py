"""Publication client and the five transfer checks, using the leased SSH client."""
import hashlib
import re
import shlex
import subprocess
import time

OLD = b"previous destination\n"
BLOCK = 4 * 1024 * 1024
ABSENT = 'absent'


def bput(path, size, digest, mode):
    return shlex.join(('/bin/bput', path, str(size), digest, mode))


def remote_hash(ssh, path):
    script = f'p={shlex.quote(path)}; ' + (
        'if [ -f "$p" ] && [ ! -L "$p" ]; then sha256sum < "$p"; '
        'elif [ ! -e "$p" ] && [ ! -L "$p" ]; then printf "absent\\n"; else exit 1; fi'
    )
    try:
        reply = ssh(script)
    except (OSError, ValueError, subprocess.SubprocessError):
        return None
    if reply.returncode or reply.stderr:
        return None
    if reply.stdout == b'absent\n':
        return ABSENT
    match = re.fullmatch(rb'([a-f0-9]{64})  -\n', reply.stdout)
    return match[1].decode() if match else None


def deliver(ssh, artifact, dest, facts, mode):
    """Upload once and classify reconnection using the old and new digests."""
    prior = remote_hash(ssh, dest)
    if prior is None:
        return 1, 'not published; cannot establish the old destination digest'
    digest = facts['sha256']
    try:
        with artifact.open('rb') as source:
            reply = ssh(bput(dest, facts['size'], digest, mode), source=source)
    except (OSError, ValueError, subprocess.SubprocessError):
        reply = None
    acknowledged = reply is not None and reply.returncode in (0, 3)
    if reply is not None and reply.returncode == 1:
        refusal = re.search(rb'^bput: not published; (.+)$', reply.stderr, re.M)
        if refusal and not reply.stdout:
            return 1, 'not published; ' + refusal[1].decode(errors='replace')

    # A fresh SSH transport reads DEST. If a successful status was followed by
    # an unavailable readback, try one further connection within the lease.
    actual = remote_hash(ssh, dest)
    if acknowledged and actual != digest:
        actual = remote_hash(ssh, dest)
    if actual == digest:
        if reply is not None and reply.returncode == 3:
            return 3, 'published; directory sync failed'
        return 0, 'published'
    if actual == prior:
        return 1, 'not published'
    return 2, 'unknown; destination digest could not be reconciled'


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def expect(reply, status=0, stdout=b'', stderr=b''):
    require((reply.returncode, reply.stdout, reply.stderr) == (status, stdout, stderr),
            f'SSH status {reply.returncode}, stdout {reply.stdout[:160]!r}, stderr {reply.stderr[:160]!r}')


class TransferCases:
    def __init__(self, client, known, nonce, uid, gid, artifact, facts):
        self.client, self.known = client, known
        self.home = f'/home/{client.user}'
        self.uid, self.gid = uid, gid
        self.work = f'.transfer-{nonce}'
        self.artifact, self.facts = artifact, facts
        self.lease_deadline = client.deadline

    def ssh(self, script, **options):
        return self.client.ssh(self.known, f'cd {self.work} && {script}', **options)

    def run(self):
        expect(self.client.ssh(self.known, f'mkdir -m 700 {self.work}'))
        cases = []
        try:
            for case in (self.mac_artifact_published, self.bad_checksum_refused,
                         self.interrupted_then_reconnect, self.backpressure_exact,
                         self.publish_faults_located):
                started = time.monotonic()
                self.client.deadline = min(self.lease_deadline, started + 10)
                row = dict(id='transfer/' + case.__name__.replace('_', '-'))
                try:
                    detail = case()
                    row.update(result='PASS', **(detail or {}))
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    row.update(result='FAIL', reason=str(error))
                cases.append(row)
                print(f'{row["result"]} {row["id"]}' + (': ' + row['reason'] if 'reason' in row else ''),
                      flush=True)
        finally:
            self.client.deadline = self.lease_deadline
            expect(self.client.ssh(self.known, f'chmod 700 {self.work}/locked 2>/dev/null; rm -rf {self.work}'))
        return cases

    def mac_artifact_published(self):
        status, outcome = deliver(self.ssh, self.artifact, 'mac-artifact', self.facts, '755')
        require(status == 0, outcome)
        identity = self.ssh('id -u; id -g; echo "$HOME"')
        expect(identity, stdout=f'{self.uid}\n{self.gid}\n{self.home}\n'.encode())
        reply = self.ssh('./mac-artifact; rc=$?; sha256sum < mac-artifact || exit 1; exit "$rc"')
        digest = self.facts['sha256']
        expect(reply, status=42, stdout=f'transfer stdout\n{digest}  -\n'.encode(),
               stderr=b'transfer stderr\n')
        executed = reply.stdout.splitlines()[-1].split()[0].decode()
        return dict(executedSha256=executed, uid=self.uid, gid=self.gid, home=self.home, status=reply.returncode)

    def seed(self, path):
        expect(self.ssh(f'cat > {shlex.quote(path)}', data=OLD))

    def no_temp(self, directory='.'):
        return ('for temp in ' + shlex.quote(directory) + '/.*.bput.*; do '
                'if [ -e "$temp" ] || [ -L "$temp" ]; then exit 42; fi; done; ')

    def refusal(self, reply, reason):
        require(reply.returncode == 1 and not reply.stdout
                and b'bput: not published; ' + reason in reply.stderr,
                f'expected a located publication refusal, got status {reply.returncode}: {reply.stderr[:200]!r}')

    def bad_checksum_refused(self):
        self.seed('bad')
        reply = self.ssh(bput('bad', 3, '0' * 64, '755'), data=b'abc')
        self.refusal(reply, b'SHA-256 mismatch')
        expect(self.ssh(self.no_temp() + 'cat bad'), stdout=OLD)

    def interrupted_then_reconnect(self):
        self.seed('interrupted')
        require(self.facts['size'] > 3, 'transfer artifact has no remainder for the interrupted upload')
        with self.artifact.open('rb') as source:
            prefix = source.read(3)
        script = bput('interrupted', self.facts['size'], self.facts['sha256'], '755')
        received = self.client.interrupt(self.known, f'cd {self.work} && {script}', prefix)
        while True:
            reply = self.ssh(self.no_temp() + 'cat interrupted')
            if reply.returncode != 42:
                expect(reply, stdout=OLD)
                break
            time.sleep(0.05)
        status, outcome = deliver(self.ssh, self.artifact, 'interrupted', self.facts, '755')
        require(status == 0, outcome)
        return dict(interruptedAfter=received, publishedSha256=self.facts['sha256'])

    def backpressure_exact(self):
        body = bytes(range(256)) * (BLOCK // 256)
        reply = self.ssh(f'head -c {BLOCK} /dev/zero && cat && exit 23', data=body)
        expect(reply, status=23, stdout=bytes(BLOCK) + body)

    def publish_faults_located(self):
        expect(self.ssh('mkdir locked as-dir'))
        self.seed('locked/dest')
        self.seed('as-dir/sentinel')
        expect(self.ssh('chmod 500 locked'))
        for dest, operation in [('locked/dest', b'open temporary file'), ('as-dir', b'rename')]:
            reply = self.ssh(bput(dest, 3, hashlib.sha256(b'abc').hexdigest(), '755'), data=b'abc')
            self.refusal(reply, operation)
        expect(self.ssh(self.no_temp() + self.no_temp('locked') + self.no_temp('as-dir') +
                        'test -d as-dir && cat locked/dest as-dir/sentinel'), stdout=OLD * 2)
