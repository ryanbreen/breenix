"""M6.1 accepts executed A06F work, never PUT or a seeded completion alone."""
import copy
import json
from pathlib import Path
import runpy
import unittest
ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
BASE = runpy.run_path(str(ROOT / 'tests/gt710_memory_report_test.py'))
TITLE = 'A privileged A06F channel executes unique semaphores with native identity, epoch and GET/PUT progression; withheld and stale completions fail'


def fixture():
    e = dict(schema=1, bdf='01:00.0', chipset=0x106, host_class=0xa06f,
             channel=0, epoch=1, instance=256<<20, directory=(256<<20)+12288,
             userd=257<<20, ring=258<<20, push=259<<20, semaphore=260<<20,
             top=[0x80000036 | (2<<21) | (4<<26), 7]+[0]*62,
             pbdmas=[[0,4]], runlist=2, engine=4, bind=0x80010000,
             state=0x20001, fault=0, pbdma_errors=[0,0],
             samples=[dict(sequence=i,epoch=1,expected=0x12345000+i,
                           before=0,after=0x12345000+i,get_before=i-1,put=i,get=i,
                           reference=0x12345000+i) for i in (1,2)],
             withheld=dict(epoch=1,expected=0x12345003,observed=0,get=2,put=2,
                           elapsed_us=10000,accepted=False),
             stale=dict(epoch=0,expected=0x12345002,observed=0x12345002,
                        get=2,put=2,accepted=False),
             drain=dict(runlist_count=0,pending=0,preempt=0,binding=0),
             retained_on_failure=True, firmware_writes=0, ce='pending-6.2',
             vm_probes='pending-6.2', reset='pending-6.5')
    e['progress']=dict(source='pbdma-live',pbdma=0,channel=0x2000,userd_low=e['userd'],userd_high=0)
    e['suppressed']=dict(epoch=1,expected=0x12345003,observed=0,get=3,put=3,
                         reference=0x12345003,elapsed_us=10000,accepted=False)
    e['clean']=dict(sequence=4,epoch=1,expected=0x12345004,before=0,after=0x12345004,
                    get_before=3,put=4,get=4,reference=0x12345004)
    return BASE['SERIAL'], e


class ChannelCapture(unittest.TestCase):
    def test_executed_channel_and_negative_controls(self):
        serial,e=fixture()
        def check(e): return API['privileged_channel'](serial+'GT710_CHANNEL '+json.dumps(e)+'\n')
        check(e)
        for change in [lambda b:b.update(host_class=0xa16f),lambda b:b.update(bdf='02:00.0'),
                       lambda b:b.update(epoch=0),lambda b:b.update(top=[]),
                       lambda b:b.update(pbdmas=[[0,0]]),lambda b:b.update(fault=1),
                       lambda b:b.update(bind=0),lambda b:b.update(pbdma_errors=[1,0]),
                       lambda b:b['samples'][0].update(after=0),
                       lambda b:b['samples'][0].update(before=b['samples'][0]['expected']),
                       lambda b:b['samples'][0].update(get=0),lambda b:b['samples'][1].update(epoch=0),
                       lambda b:b['samples'][1].update(expected=b['samples'][0]['expected']),
                       lambda b:b['samples'][1].update(reference=0),lambda b:b['samples'].pop(),
                       lambda b:b['withheld'].update(accepted=True),lambda b:b['withheld'].update(observed=0x12345003),
                       lambda b:b['withheld'].update(elapsed_us=0),lambda b:b['stale'].update(accepted=True),
                       lambda b:b['stale'].update(epoch=1),lambda b:b['drain'].update(pending=0x100000),
                       lambda b:b.update(retained_on_failure=False),lambda b:b.update(vm_probes='passed')]:
            bad=copy.deepcopy(e);change(bad)
            with self.assertRaises(ValueError): check(bad)

    def test_advanced_get_without_release_still_times_out_and_clean_next_executes(self):
        serial,e=fixture()
        def check(e): return API['privileged_channel'](serial+'GT710_CHANNEL '+json.dumps(e)+'\n')
        check(e)
        for mutate in [lambda b:b.pop('suppressed'),lambda b:b['suppressed'].update(accepted=True),
                       lambda b:b['suppressed'].update(observed=b['suppressed']['expected']),
                       lambda b:b['suppressed'].update(get=2),lambda b:b['suppressed'].update(elapsed_us=0),
                       lambda b:b.pop('clean'),lambda b:b['clean'].update(after=0),
                       lambda b:b['clean'].update(epoch=0),lambda b:b['clean'].update(get=3),
                       lambda b:b['clean'].update(before=b['clean']['expected'])]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)

    def test_live_progress_belongs_to_the_selected_channel_userd(self):
        serial,e=fixture()
        def check(e): return API['privileged_channel'](serial+'GT710_CHANNEL '+json.dumps(e)+'\n')
        check(e)
        for mutate in [lambda b:b.pop('progress'),lambda b:b['progress'].update(source='software'),
                       lambda b:b['progress'].update(pbdma=1),lambda b:b['progress'].update(channel=1),
                       lambda b:b['progress'].update(userd_low=b['push']),lambda b:b['progress'].update(userd_high=1)]:
            bad=copy.deepcopy(e);mutate(bad)
            with self.assertRaises(ValueError):check(bad)

    def test_channel_case_has_a_metal_measurement(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text());API['validate_manifest'](manifest)
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='channel')['cases'],[dict(id='semaphore',title=TITLE)])
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='channel')['measure'],dict(suite='gt710',categories=['channel']))
        score=runpy.run_path(str(ROOT/'scripts/metal/metal_score.py'))
        self.assertEqual(score['HOST_MILESTONES']['gt710-channel.json'],'metal-gt710/channel')

if __name__=='__main__': unittest.main()
