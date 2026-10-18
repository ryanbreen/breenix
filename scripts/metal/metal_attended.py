"""The fixed, human-typed console challenge. This module never sends input."""

LOGIN_STAGES = (
    'Getty on the console', 'Login prompt', 'Wrong password refused',
    'USB keyboard login accepted', 'Logged-in computation answered',
    'USB keyboard session ended', 'Getty reacquired the console after logout',
    'Login prompt after logout',
)


class Challenge:
    def __init__(self):
        self.reached = 0
        self.refused = False
        self.ended = False
        self.invalid = False

    @property
    def done(self):
        return self.reached == len(LOGIN_STAGES) and not self.invalid

    def feed(self, line):
        # claim-lint:ok: source mutation cases in tests/metal_attended_test.py.
        # These markers come from TTY provenance, not from echoed commands or
        # the input transport selected in run.json. Mixed input invalidates proof.
        if line in ('[console] login refused input=other', '[console] login accepted input=other',
                    '[console] session ended input=other'):
            self.invalid = True
        if line == '[console] login refused input=keyboard':
            if self.refused or self.reached != 2:
                self.invalid = True
            self.refused = True
        checks = (
            line == '[init] getty on the console',
            line.startswith('Breenix login: '),
            line == 'Login incorrect' and self.refused,
            line == '[console] login accepted input=keyboard',
            line == 'metal-console-ok 42',
            line == '[console] session ended input=keyboard' and self.ended,
            line == '[getty] console acquired',
            line.startswith('Breenix login: '),
        )
        if self.reached == 5 and line == '[init] console session ended; restarting getty':
            self.ended = True
        if not self.invalid and self.reached < len(checks) and checks[self.reached]:
            self.reached += 1
        return self.done

    def verdict(self):
        reached = 0 if self.invalid else self.reached
        return {'reached': reached, 'total': len(LOGIN_STAGES), 'passed': self.done}
