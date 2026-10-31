"""K7 live stop decisions for the composed clock diagnostic."""
import importlib.util
import json
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('c6_watch', Path(__file__).resolve().parents[1] / 'scripts/metal/gt710_c6_watch.py')
watch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(watch)

class C6Watch(unittest.TestCase):
    def test_counter_and_readback_one_percent_boundaries(self):
        for target in (50625, 81653):
            for delta in (-target//100+1, 0, target//100):
                line='GT710_CLOCK_MEASURED '+json.dumps(dict(status='ok',khz=target+delta,counter_restored=True))
                self.assertIsNone(watch.stop_reason(line,self.fail,gpc_target=target))
            line='GT710_CLOCK_MEASURED '+json.dumps(dict(status='ok',khz=target+target//100+1,counter_restored=True))
            self.assertEqual(watch.stop_reason(line,self.fail,gpc_target=target),'gpc-counter-outside-1-percent')
        for target in (810000,1307000):
            line='GT710_CLOCK_READ '+json.dumps(dict(status='ok',readback_khz=target))
            self.assertIsNone(watch.stop_reason(line,self.fail,gpc_target=(target+15)//16))
        for target in (405000,900000):
            for value in (target-target//100,target+target//100):
                line='GT710_MEMORY_READ '+json.dumps(dict(readback_khz=value,post_khz=405000))
                self.assertIsNone(watch.stop_reason(line,self.fail,target=target))
            line='GT710_MEMORY_READ '+json.dumps(dict(readback_khz=target+target//100+1,post_khz=405000))
            self.assertEqual(watch.stop_reason(line,self.fail,target=target),'memory-clock-outside-1-percent')

    def test_faults_temperature_done_and_observation_errors(self):
        for line,expected in (
            ('GT710_C6_DONE','C6-complete'),
            ('GT710_PLL_LOCK {"locked":false}','PLL-lock-timeout'),
            ('GT710_C6_ERROR console mismatch','gpu-fault-or-failed-step'),
            ('GT710_NATIVE_FAULT reason=fresh underflow','gpu-fault-or-failed-step'),
            ('GT710_MEMX_EXEC_REPLY phase=target status=-1 reason=no-reply','EXEC-failed'),
            ('GT710_BENCH {"completion":false}','completion-failed'),
            ('GT710_TEMPERATURE {"celsius":85}','temperature-85C'),
            ('GT710_TEMPERATURE {"celsius":84}',None),
            ('GT710_BENCH {"completion":true,"elapsed_ns":9999999999}',None),
            ('GT710_MEMX_EXEC_REPLY phase=target status=0 pause_ns=136000',None),
            ('GT710_TEMPERATURE reading={"value":null}',None),
        ):
            self.assertEqual(watch.stop_reason(line,self.fail),expected)
        logs=[]
        self.assertIsNone(watch.stop_reason('GT710_CLOCK_MEASURED {broken',logs.append))
        self.assertEqual(len(logs),1)
        self.assertIn('WATCH_PARSE_ERROR',logs[0])

if __name__=='__main__':unittest.main()
