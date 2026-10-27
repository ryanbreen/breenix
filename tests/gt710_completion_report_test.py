"""Native records bind completed transfers to resource versions and caller fences."""
import copy
from pathlib import Path
import runpy
import unittest
API=runpy.run_path(str(Path(__file__).resolve().parents[1]/'scripts/metal/gt710-suite'))

class NativeObservation(unittest.TestCase):
    def test_foreign_or_stale_record_and_virtual_success_are_refused(self):
        case=dict(id=0,instance=0x123000,epoch=17,dest=0x456000,size=8192,releases=[2,3],sequence=3)
        request=lambda seq,caller:dict(context=case['instance'],epoch=17,resource=case['dest'],generation=9,sequence=seq,caller=caller)
        record=dict(schema=1,id=0,kind='a0b5-engine-release',context=case['instance'],epoch=17,resource=case['dest'],generation=9,
                    writer=request(1,9001),transfers=[request(2,23),request(3,24)],bytes=8192,unwritten=0)
        check=API['observation_record']
        check(record,case)
        for field,value in [('context',0x124000),('epoch',16),('resource',0x457000),('generation',10),
                            ('bytes',8191),('unwritten',1),('kind',0x1100)]:
            bad=copy.deepcopy(record);bad[field]=value
            with self.assertRaises(ValueError):check(bad,case)
        for group in ['writer','first','second']:
            for field,value in [('context',0x124000),('epoch',16),('resource',0x457000),('generation',8),('caller',0)]:
                bad=copy.deepcopy(record)
                req=bad['writer'] if group=='writer' else bad['transfers'][0 if group=='first' else 1]
                req[field]=value
                with self.assertRaises(ValueError):check(bad,case)
        for transfers in [[record['transfers'][0]]*2,list(reversed(record['transfers'])),[record['writer'],record['transfers'][1]]]:
            bad=copy.deepcopy(record);bad['transfers']=transfers
            with self.assertRaises(ValueError):check(bad,case)
        with self.assertRaises(ValueError):API['native_observations']('')

if __name__=='__main__':unittest.main()
