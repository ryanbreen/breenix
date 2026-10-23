"""Remote-upgrade case assertions; inputs are retained receipts from actual installs.

No console prompt, staged slot, or unexecuted command substitutes for S21's probe.
"""


def probe(receipt, generation):
    if (receipt.get('generation') != generation or not receipt.get('authenticated')
            or not receipt.get('strictHostKey') or not receipt.get('servicesStarted')
            or not 0 <= receipt.get('completedAt', 0) - receipt.get('powerOn', 0) <= 600
            or receipt.get('completedAt', 0) <= receipt.get('powerOn', 0)):
        raise ValueError('S21 needs strict operator SSH and started services within 600 s of power-on')


def upgrade(result, broken, baseline):
    if result.get('outcome') != ('rolled-back' if broken else 'verified'):
        raise ValueError('install-persistent did not complete the required automatic outcome')
    if result.get('toolsCommit') != baseline['commit']:
        raise ValueError('upgrade used different metal tools')
    receipt = result.get('rollback' if broken else 'verification', {})
    if receipt.get('fingerprint') != baseline['fingerprint']:
        raise ValueError('S21 probe observed a changed host key')
    if broken:
        if result.get('verification', {}).get('sshAttempts', 0) < 1:
            raise ValueError('the broken candidate received no actual SSH probe attempt')
        if result.get('verification', {}).get('authenticated'):
            raise ValueError('the deliberately broken generation answered SSH')
        if result['candidateGeneration'] == result['previousGeneration']:
            raise ValueError('the fault did not exercise a new generation')
        probe(result.get('rollback', {}), result['previousGeneration'])
    else:
        probe(result.get('verification', {}), result['candidateGeneration'])
    for state in result.get('observations', []):
        if any(state.get(k) != baseline[k] for k in ('fingerprint', 'rootSentinel', 'homeSentinel')):
            raise ValueError('upgrade changed host key or persistent sentinels')
    if len(result.get('observations', [])) < (2 if broken else 1):
        raise ValueError('missing before/after machine-data observations')


def generations(proof, reclamation=False):
    names = ('reclamation',) if reclamation else ('enospc', 'io', 'interruption', 'paired-rollback')
    for name in names:
        if proof.get(name, {}).get('exit') != 0 or not proof[name].get('executed'):
            raise ValueError('missing executed generation proof: ' + name)
