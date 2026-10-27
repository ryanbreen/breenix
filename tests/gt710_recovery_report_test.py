"""Fault/reset acceptance must include hardware removal, holds and fresh work."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import unittest
API=runpy.run_path(str(Path(__file__).resolve().parents[1]/'scripts/metal/gt710-suite'))
class Recovery(unittest.TestCase):
    def test_fault_and_watchdog_require_hardware_proof(self):
        seed=12345
        digest=lambda s:hashlib.sha256(bytes(API['ce_pattern'](i,s) for i in range(8192))).hexdigest()
        report=dict(schema=1,seed=seed,worker=True,poisons=[85,170],baseline=[1,2,3],after=[1,2,3],firmware_writes=0,
            fault=dict(instance=0x300000,invalid=0x403000,raw=[0x300,0x403000,0,2],mask=1<<21,
                reset_bit=6,reset_disabled=0,reset_enabled=64,pending=0,busy=0,errors=0,held=1,rejected=True,removed=True,runlist=[1],binding=0,survivor_context=0x400000,
                survivor_epoch=33,survivor_after_context=0x400000,survivor_after_epoch=33,
                survivor_digests=[digest(seed^0xffffffff)]*2,reset_us=60,confirmed=True),
            watchdog=dict(reset_bit=6,reset_disabled=0,reset_enabled=64,pending=0,busy=0,errors=0,quarantine_us=2_000_010,no_progress_us=2_000_010,total_us=4_020_000,
                held_at_quarantine=1,held_before_reset=[1,1],failed_reset_held=[1,1],rejected=True,
                removed=True,runlist=[1],binding=0,success_before_reset=False,hardware_completed=[False,False],hardware_fences=[[0,0,0],[0,0,0]],expected_fences=[[1,44,0],[3,33,0]],failed_reset_rejected=True,failed_reset_observation=[64,0,0x80000400],
                survivor_digests=[digest(seed^0xffffffff)]*2,reset_us=1500,confirmed=True,
                old_epochs=[44,33],new_epochs=[55,66],old_fences_eio=[True,True],stale_accepted=False,
                bindings=[0x500000,0x600000],fresh_digests=[[digest(seed)]*2,[digest(seed^0xffffffff)]*2]))
        report['active_reset']=dict(busy=0x80000000,word=12345,nonce=12345,held=1,reset_us=60,reset_disabled=0,reset_enabled=64,reset_bit=6,fresh_digests=[digest(seed^0xffffffff)]*2)
        line=lambda e:'GT710_RECOVERY '+json.dumps(e)+'\n'
        API['recovery_gate'](line(report))
        for section,field,value in [('', 'worker',False),('active_reset','busy',0),('active_reset','word',0),('active_reset','reset_us',100001),('fault','raw',[0,0,0,2]),
                ('fault','reset_disabled',64),('fault','busy',1),('fault','binding',1),('fault','runlist',[0,1]),('fault','held',0),
                ('fault','survivor_digests',['0'*64]*2),('fault','survivor_after_epoch',34),
                ('watchdog','quarantine_us',1999999),('watchdog','no_progress_us',1999999),
                ('watchdog','hardware_completed',[False,True]),('watchdog','hardware_fences',[[1,44,0],[3,33,0]]),('watchdog','failed_reset_rejected',False),('watchdog','failed_reset_observation',[0,0,0]),
                ('watchdog','total_us',4850001),('watchdog','held_before_reset',[0,1]),
                ('watchdog','failed_reset_held',[0,1]),('watchdog','reset_us',100001),
                ('watchdog','reset_enabled',0),('watchdog','pending',0x100000),('watchdog','errors',1),('watchdog','confirmed',False),('watchdog','old_fences_eio',[True,False]),
                ('watchdog','new_epochs',[44,33]),('watchdog','stale_accepted',True),
                ('watchdog','fresh_digests',[['0'*64]*2,['0'*64]*2])]:
            bad=copy.deepcopy(report);(bad[section] if section else bad)[field]=value
            with self.assertRaises(ValueError,msg=field):API['recovery_gate'](line(bad))
        self.assertEqual(API['CATEGORIES']['recovery'][0][0],'fault-reset')
if __name__=='__main__':unittest.main()
