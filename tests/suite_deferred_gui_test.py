"""Deferred GUI runners keep result transport while postponing display stages."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
RUN = SOURCE.split('pub fn run_with_finalizer(', 1)[1].split('// Launcher-supplied HTTP route.', 1)[0]


def condition_before(fragment):
    return re.search(r'if ([^\{]+)\{' + re.escape(fragment), RUN).group(1).strip()


def selected(condition, gui, display):
    expression = condition.replace('self.gui', str(gui)).replace('screen.gui', str(display))
    expression = expression.replace('&&', ' and ').replace('!', 'not ')
    return eval(expression, {'__builtins__': {}}, {'recording': True})


class DeferredGui(unittest.TestCase):
    def test_display_work_follows_the_active_screen(self):
        conditions = [
            condition_before('crate::suite_stage::init(self)'),
            re.search(r'let baseline_ready = if ([^\{]+)\{', RUN).group(1),
            re.search(r'let retired = if ([^\{]+)\{', RUN).group(1),
            # The production block that records and completes each visual stage.
            re.search(r'if ([^\{]+)\{\s*// Use the exact emitted VALUE records', RUN).group(1),
        ]
        for condition in conditions:
            with self.subTest(condition=condition):
                self.assertFalse(selected(condition, True, False), 'deferred runner painted before its final panel')
                self.assertTrue(selected(condition, True, True))
                self.assertFalse(selected(condition, False, False))

    def test_final_text_panel_uses_the_active_screen(self):
        condition = condition_before(' screen.draw(self, &title, &subtitle, &states, verdict, &feed, only); ')
        self.assertTrue(selected(condition, True, False), 'deferred GUI final panel was omitted')
        self.assertFalse(selected(condition, True, True))
        # GUI metadata still enables the result pipe, including with no live display.
        self.assertIn('run_case(case, limit_ms, self.id, &name, self.gui,', RUN)
        self.assertIn('let mut results = if gui {', SOURCE)


if __name__ == '__main__':
    unittest.main()
