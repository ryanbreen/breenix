"""Reject incomplete native concurrency and killed-wait evidence."""
import copy
import json
from pathlib import Path
import runpy
import unittest
API=runpy.run_path(str(Path(__file__).resolve().parents[1]/'scripts/metal/gt710-suite'))
class Multi(unittest.TestCase):
    def test_full_opposite_readbacks_wrap_and_retention(self):
        seed=0x12345678
        import hashlib
        digest=lambda s:hashlib.sha256(bytes(API['ce_pattern'](i,s) for i in range(8192))).hexdigest()
        e=dict(schema=1,seed=seed,channels=[dict(id=i,instance=0x200000+i*0x10000,
            commands=0x300000+i*0x10000,source=0x400000+i*0x10000,dest=0x500000+i*0x10000,
            epoch=42+i,first=1,last=521,backpressure=130,compares=130,
            poisons=[85,170],digests=[digest(seed if i==0 else seed^0xffffffff)]*130) for i in range(2)],
            baseline=[1,2,3],after=[1,2,3],firmware_writes=0)
        serial=lambda v:'GT710_MULTI '+json.dumps(v)+'\n'
        API['multiple_contexts'](serial(e))
        for field,value in [('commands',e['channels'][0]['commands']),('epoch',0),('last',511),
                            ('backpressure',0),('compares',129),('digests',['0'*64]*130)]:
            bad=copy.deepcopy(e);bad['channels'][1][field]=value
            with self.assertRaises(ValueError):API['multiple_contexts'](serial(bad))
        k=dict(schema=1,waiter=7,held_before=1,held_after_kill=1,released_before_drain=0,
               killed=True,kill_us=1234,drained=True,after_baseline=True,survivor=True,
               device_completed_before_kill=False,complete_compares=2,poisons=[85,170])
        line=lambda v:'GT710_KILLED_WAIT '+json.dumps(v)+'\n'
        API['killed_wait'](line(k))
        for field,value in [('held_after_kill',0),('released_before_drain',1),('kill_us',250001),
                            ('killed',False),('drained',False),('after_baseline',False),
                            ('device_completed_before_kill',True),('survivor',False),('complete_compares',1)]:
            bad=copy.deepcopy(k);bad[field]=value
            with self.assertRaises(ValueError):API['killed_wait'](line(bad))
if __name__=='__main__':unittest.main()
