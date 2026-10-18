"""The fixed, human-typed console challenge. This module never sends input.

The stages are console-login's `attendedStages` in docs/metal/boot-path.json, matched in order, so the driver and
the scorer read one list. Any number of refused attempts may come before the accepted one; at least one refusal
must. A provenance line naming input other than the USB keyboard invalidates the challenge.
"""
import re

# claim-lint:ok: source mutation cases in tests/metal_attended_test.py.
# These come from the kernel's TTY provenance report, not from echoed commands or run.json's input transport.
OTHER_INPUT = re.compile(r'\[console\] (login refused|login accepted|session ended) input=other')


def stages(definition):
    for milestone in definition['milestones']:
        if milestone['id'] == 'console-login':
            return milestone['attendedStages']
    raise ValueError('the definition has no console-login milestone')


def matches(stage, line):
    alternatives = stage['marker'].split('|')
    if stage.get('match') == 'line':
        return line in alternatives
    return any(marker in line for marker in alternatives)


class Challenge:
    def __init__(self, stages):
        self.stages = stages
        self.reached = 0
        self.invalid = False

    @property
    def done(self):
        return self.reached == len(self.stages) and not self.invalid

    def feed(self, line):
        """Feed complete lines, or a prompt still waiting for its newline; a repeat cannot advance twice."""
        if OTHER_INPUT.search(line):
            self.invalid = True
        if not self.invalid and self.reached < len(self.stages) and matches(self.stages[self.reached], line):
            self.reached += 1
        return self.done

    def verdict(self):
        reached = 0 if self.invalid else self.reached
        result = {'reached': reached, 'total': len(self.stages), 'passed': self.done}
        result['missing'] = [stage['name'] for stage in self.stages[reached:]]
        if self.invalid:
            result['invalid'] = 'input other than the USB keyboard reached the console'
        return result
