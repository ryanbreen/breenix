"""S20's machine-event assertions. Receipts come from the real boot loop."""
import re

CASES = (
    ('clean-preempt', 'A queued test preempts the persistent session only after a clean shutdown'),
    ('idle-return', 'An empty test queue returns to the current persistent install without a caller boot'),
    ('one-shot', 'A one-shot previous entry leaves current as the next persistent default'),
    ('reboot-return', 'An authenticated reboot returns unaided to the current persistent install'),
    ('watchdog-return', 'A watchdog hang returns unaided to current persistent SSH within 600 seconds'),
    ('uptime-24h', 'One persistent session answers strict authenticated SSH every five minutes for 24 hours'),
)


def persistent(run, sha):
    if (run.get('bootedCommit'), run.get('toolsCommit'), run.get('mode'), run.get('evidenceEligible')) != (sha, sha, 'persistent', True):
        raise ValueError('persistent evidence has the wrong kernel, tools or boot kind')


def clean(receipt, sha):
    persistent(receipt['run'], sha)
    request = receipt['preemption']
    if (request.get('waiter') != receipt['test'] or request.get('complete') is not True
            or request.get('authenticated') is not True or request.get('strictHostKey') is not True):
        raise ValueError('queued test has no authenticated preemption request')
    if receipt['exit'] != '0':
        raise ValueError('preempted session did not end normally')
    events = receipt['events']
    if not any(e['event'] == 'requested-shutdown' and e.get('action') == 'poweroff' for e in events):
        raise ValueError('preemption has no authenticated clean power-off receipt')
    for mount in ('root', 'home'):
        if '[shutdown] ' + mount + ' clean read-only' not in receipt['serial']:
            raise ValueError('preemption did not make ' + mount + ' clean read-only')
    shutdown = next(e['t'] for e in events if e['event'] == 'requested-shutdown')
    if shutdown <= receipt['queuedAt'] or receipt['testClaimedAt'] <= shutdown:
        raise ValueError('test did not wait for the queued clean shutdown')
    if any(e['event'] in ('nmi', 'driver-error', 'console-gap-start', 'reaper') for e in events):
        raise ValueError('preemption used recovery or lost capture')


def returned(receipt, sha):
    persistent(receipt['run'], sha)
    if not receipt['run'].get('resting') or receipt['entry'] != 'current':
        raise ValueError('idle return was not the lab current-default boot')
    if receipt['boot'] == receipt['source'] or receipt['startedAt'] <= receipt['sourceEndedAt']:
        raise ValueError('idle return did not follow the completed test')
    if not receipt['authenticated'] or not receipt['strictHostKey'] or not receipt['queueEmpty']:
        raise ValueError('idle return has no empty queue and authenticated SSH proof')
    for mount in ('root', 'home'):
        if '[storage] ' + mount + ' prior state=clean' not in receipt['serial']:
            raise ValueError('idle persistent mount was not clean')


def one_shot(receipt, sha):
    if receipt['onceRun'].get('toolsCommit') != sha or receipt['onceEntry'] != 'previous':
        raise ValueError('one-shot never booted previous with the candidate tools')
    if receipt['beforeSlots'] != receipt['afterSlots']:
        raise ValueError('one-shot changed the default slots')
    persistent(receipt['resetRun'], sha)
    if receipt['resetEntries'] != ['previous', 'current'] or receipt['beforeId'] == receipt['afterId']:
        raise ValueError('a reset in the same lease repeated the one-shot choice')
    returned(receipt['returned'], sha)


def reboot(receipt, sha):
    persistent(receipt['beforeRun'], sha)
    persistent(receipt['afterRun'], sha)
    if receipt['beforeId'] == receipt['afterId'] or receipt['entry'] != 'current':
        raise ValueError('reboot did not produce a fresh current persistent boot')
    names = [e['event'] for e in receipt['events']]
    if 'requested-reset-observed' not in names or 'requested-reboot-ready' not in names:
        raise ValueError('reboot did not return autonomously')
    if any(n in names for n in ('power-on', 'power-off', 'nmi', 'console-gap-start', 'driver-error')):
        raise ValueError('reboot needed host recovery or lost capture')
    if not receipt['authenticated'] or not receipt['strictHostKey']:
        raise ValueError('reboot has no authenticated SSH receipt')


def watchdog(receipt, sha):
    if receipt.get('passed') is not True or receipt.get('commit') != sha:
        raise ValueError('watchdog recovery did not pass at this commit')
    ssh = receipt['receipt']
    if ssh['case'] != 'feeder-hang' or not 0 < ssh['sshAt'] - ssh['trigger'] <= 600:
        raise ValueError('watchdog did not return unaided within S19 deadline')


def uptime(receipt, sha):
    persistent(receipt['run'], sha)
    samples = receipt['samples']
    if len(samples) != 289:
        raise ValueError('24-hour proof requires the initial probe and 288 five-minute probes')
    if receipt['elapsedMonotonic'] < 86400 or samples[-1]['at'] - samples[0]['at'] < 86400:
        raise ValueError('persistent observation was shorter than 24 hours')
    identity = samples[0]['bootId']
    if not re.fullmatch(r'[0-9a-f]+-[0-9a-f]+-[0-9a-f]+', identity):
        raise ValueError('missing guest boot identity')
    for i, sample in enumerate(samples):
        if (sample['bootId'] != identity or not sample['authenticated'] or not sample['strictHostKey']
                or sample['lockHeld'] or sample['lease'] != receipt['boot']):
            raise ValueError('persistent session changed, SSH failed or branch-tools lock was held')
        if i and not 295 <= sample['at'] - samples[i - 1]['at'] <= 305:
            raise ValueError('a five-minute probe was missed')


def validate(records, sha):
    clean(records['clean-preempt'], sha)
    returned(records['idle-return'], sha)
    one_shot(records['one-shot'], sha)
    reboot(records['reboot-return'], sha)
    watchdog(records['watchdog-return'], sha)
    uptime(records['uptime-24h'], sha)
