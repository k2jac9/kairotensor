"""Offline diagnostics: real C export, resealed adversaries and no-execution checks."""
import copy
from contextlib import ExitStack, redirect_stdout, redirect_stderr
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.request

from decision_bundle_v1 import canonical_bytes, decode_envelope

ROOT = Path(__file__).resolve().parents[1]


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def envelope(raw):
    return dict(hex=raw.hex(), byte_len=len(raw), sha256=sha(raw))


def seal(bundle):
    bundle = copy.deepcopy(bundle)
    bundle['integrity'].pop('sha256', None)
    bundle['integrity']['sha256'] = sha(canonical_bytes(bundle))
    return bundle


def rebuild(bundle, count=None, identical=False):
    """Create explicitly synthetic declared-contract variants of a real C export."""
    b = copy.deepcopy(bundle)
    a = b['attempts'][0]
    context = json.loads(decode_envelope(b['evidence'][0]['payload']))
    m = json.loads(decode_envelope(a['immutable_manifest']))
    count = count or len(m['sources'])
    while len(m['sources']) < count:
        index = len(m['sources'])
        source = copy.deepcopy(m['sources'][0])
        source.update(external_id=f'e{index+1:02}', occurrence_id=index+2)
        m['sources'].append(source)
        e = copy.deepcopy(b['evidence'][1])
        e['occurrence_id'] = index+2
        b['evidence'].append(e)
    context['evidence_ids'] = [s['external_id'] for s in m['sources']]
    b['evidence'][0]['payload'] = envelope(canonical_bytes(context))
    m['context']['sha256'] = b['evidence'][0]['payload']['sha256']
    a['inputs'] = [dict(position=0, evidence_id=m['context']['occurrence_id'], role='context')]
    reports = []
    for i,s in enumerate(m['sources'], 1):
        e = b['evidence'][i]
        if identical:
            e['payload'] = copy.deepcopy(b['evidence'][1]['payload'])
        s['payload_sha256'] = e['payload']['sha256']
        a['inputs'].append(dict(position=i, evidence_id=s['occurrence_id'], role='report'))
        reports.append(dict(id=s['external_id'], text=decode_envelope(e['payload']).decode()))
    a['candidates'] = [dict(position=i['position'], evidence_id=i['evidence_id']) for i in a['inputs']]
    request = canonical_bytes(dict(messages=[dict(role='system', content=context['system_prompt']),
        dict(role='user', content=canonical_bytes(dict(question=context['question'], owner_ids=context['owner_ids'], evidence=reports)).decode())]))
    a['immutable_manifest'] = envelope(canonical_bytes(m))
    runtime = json.loads(decode_envelope(a['immutable_runtime']))
    runtime['prompt_fingerprint'] = a['immutable_manifest']['sha256']
    a['immutable_runtime'] = envelope(canonical_bytes(runtime))
    timing = json.loads(decode_envelope(a['timing_configuration']))
    timing['runtime'] = runtime
    a['timing_configuration'] = envelope(canonical_bytes(timing))
    a['dispatch_request'] = a['recovery']['request'] = envelope(request)
    # Synthetic accounting remains consistent with captured fixture-byte requests.
    a['accounting']['quote']['input_tokens'] = len(request)
    a['accounting']['charge'] = a['delivery']['usage'] = len(request) + a['delivery']['raw']['byte_len']
    return seal(b)


def outcomes(plan):
    return dict(version='decision-ablation-outcomes/1', plan_digest=sha(canonical_bytes(plan)), rows=[
        dict(trial_id=r['trial_id'], state='recorded', response_digest=sha(b'fixture response'),
             semantic_result=dict(version='recorded-normalized-result/1', value={'decision': 'abstain'}),
             charged_units=2, reserved_bound=10, unit='fixture_bytes',
             scorer=plan['scorer'], runtime=copy.deepcopy(plan['runtime']), validator=copy.deepcopy(plan['validator']))
        for r in plan['rows']])


class AblationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        runner = os.environ.get('KAIROTENSOR_DECISION_REPLAY', '')
        if not runner or not Path(runner).is_file() or not os.access(runner, os.X_OK):
            raise AssertionError('KAIROTENSOR_DECISION_REPLAY must bind a verified precompiled executable; no Cargo fallback or skips')
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder)/'real.json'
            result = subprocess.run([runner, 'export-fixture', str(path)], capture_output=True, timeout=30)
            if result.returncode:
                raise AssertionError(result.stderr)
            cls.actual = json.loads(path.read_bytes())
            replay = subprocess.run([runner, 'replay', str(path)], capture_output=True, timeout=30)
            if replay.returncode or json.loads(replay.stdout)['reconstruction'] != 'matched':
                raise AssertionError(replay.stderr)
        cls.runner = runner

    def module(self):
        self.assertTrue((ROOT/'scripts/decision_ablation.py').exists(), 'bounded offline planner/scorer must exist')
        return importlib.import_module('decision_ablation')

    def test_actual_c_export_plans_without_modification(self):
        m = self.module()
        before = canonical_bytes(self.actual)
        p = m.plan_ablations(self.actual)
        self.assertEqual(p['selected_ids'], [2,3,4])
        self.assertEqual(p['scheduled_calls'], 12)
        self.assertEqual(p['source_bundle_digest'], sha(before))
        self.assertEqual(p['runtime'], self.actual['attempts'][0]['immutable_runtime'])
        self.assertEqual(p['selected_positions'], [dict(evidence_id=i, positions=[i-1]) for i in [2,3,4]])
        self.assertEqual(before, canonical_bytes(self.actual))

    def test_four_three_pairs_exact_complete_bound(self):
        m = self.module(); b = rebuild(self.actual, 4)
        p = m.plan_ablations(b, include_pairs=True)
        self.assertEqual(p['scheduled_calls'], 33)
        self.assertEqual([r['removed_ids'] for r in p['rows'][::3]], [[],[2],[3],[4],[5],[2,3],[2,4],[2,5],[3,4],[3,5],[4,5]])
        self.assertEqual(len({r['trial_id'] for r in p['rows']}), 33)
        self.assertEqual(len({r['variant_digest'] for r in p['rows']}), 11)
        with self.assertRaises(ValueError): m.plan_ablations(b, include_pairs=True, max_calls=32)
        self.assertEqual(p, m.plan_ablations(b, include_pairs=True))

    def test_explicit_selection_threshold_order_and_ids(self):
        m = self.module(); b = rebuild(self.actual, 5)
        with self.assertRaises(ValueError): m.plan_ablations(b)
        self.assertEqual(m.plan_ablations(b, selected_ids=[5,2])['selected_ids'], [2,5])
        for ids in ([1], [999], [2,2], [True], [2,3,4,5,6], '2'):
            with self.subTest(ids=ids), self.assertRaises(ValueError): m.plan_ablations(b, selected_ids=ids)

    def test_invalid_bounds_types_overflow(self):
        m = self.module()
        for kwargs in ({'trials':0},{'trials':101},{'trials':True},{'trials':1.0}, {'trials':2**100},
                       {'max_calls':True},{'max_calls':10001},{'max_calls':0},{'include_pairs':1}, {'max_calls':1}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError): m.plan_ablations(self.actual, **kwargs)

    def test_identical_bytes_distinct_ids_and_internal_grouping(self):
        m = self.module()
        self.assertEqual(m.plan_ablations(rebuild(self.actual, identical=True))['selected_ids'], [2,3,4])
        self.assertEqual(m._group_positions([dict(position=1,evidence_id=7),dict(position=3,evidence_id=8),dict(position=5,evidence_id=7)]),
                         [dict(evidence_id=7,positions=[1,5]),dict(evidence_id=8,positions=[3])])
        b=copy.deepcopy(self.actual); b['attempts'][0]['inputs'][2]['evidence_id']=2
        with self.assertRaises(ValueError): m.plan_ablations(seal(b))

    def test_redacted_unknown_contract_roles_and_dependencies_refuse(self):
        m=self.module()
        with self.assertRaises(ValueError): m.plan_ablations(dict(status='redacted', bundle={}))
        mutations=[('inherited_from',1),('reuse_origin',1),('support',{}),('deterministic',{}),('model_contract','opaque')]
        for key,value in mutations:
            b=copy.deepcopy(self.actual); b['attempts'][0][key]=value
            with self.subTest(key=key),self.assertRaises(ValueError): m.plan_ablations(seal(b))
        for role in ('unknown','evidence','policy','context'):
            b=copy.deepcopy(self.actual); b['attempts'][0]['inputs'][1]['role']=role
            with self.subTest(role=role),self.assertRaises(ValueError):m.plan_ablations(seal(b))
        b=copy.deepcopy(self.actual);b['attempts'][0]['inputs'][0]['role']='report'
        with self.assertRaises(ValueError):m.plan_ablations(seal(b))

    def test_resealed_metadata_and_request_adversaries(self):
        m=self.module()
        def mutate_manifest(b):
            a=b['attempts'][0]; v=json.loads(decode_envelope(a['immutable_manifest']));v['sources'][0]['external_id']='forged';a['immutable_manifest']=envelope(canonical_bytes(v))
        def mutate_timing(b):
            a=b['attempts'][0];v=json.loads(decode_envelope(a['timing_configuration']));v['runtime']['threads']=True;a['timing_configuration']=envelope(canonical_bytes(v))
        changes=[lambda b:b.update(root_attempt=True),lambda b:b['snapshot'].update(root_branch=2),
                 lambda b:b['attempts'][0].update(actor='forged'),lambda b:b['evidence'][1].update(authorized_scopes=[]),
                 lambda b:b['evidence'].append(copy.deepcopy(b['evidence'][1])),
                 lambda b:b['attempts'][0]['recovery'].update(ordinal=1,predecessor_id=5),
                 lambda b:b['attempts'][0]['recovery'].update(state='unknown'),
                 lambda b:b['attempts'][0].update(dispatch_request=envelope(b'forged')),
                 lambda b:b['attempts'][0]['recovery'].update(request=envelope(b'forged')),
                 lambda b:b['attempts'][0]['validator'].update(provenance='unknown'),
                 lambda b:b['attempts'][0]['validator'].update(version='forged'), mutate_manifest,mutate_timing]
        for change in changes:
            b=copy.deepcopy(self.actual);change(b)
            with self.subTest(change=changes.index(change)),self.assertRaises(ValueError): m.plan_ablations(seal(b))

    def test_prepared_unsent_and_rejection_provenance(self):
        m=self.module();b=copy.deepcopy(self.actual);a=b['attempts'][0]
        a['dispatch_request']=None;a['recovery']['state']='reserved';a['delivery']={'state':'unresolved'}
        a['accounting']['charge']=0;a['accounting']['hold_bound']=sum(a['accounting']['quote'].values())
        self.assertEqual(m.plan_ablations(seal(b))['scheduled_calls'],12)
        a['recovery']['state']='planned'
        with self.assertRaises(ValueError):m.plan_ablations(seal(b))
        b=copy.deepcopy(self.actual);a=b['attempts'][0];a['validator']['provenance']='recorded_rejection'
        with self.assertRaises(ValueError):m.plan_ablations(seal(b))
        a['delivery'].update(disposition='invalid_output', rejection=dict(validator='incident-routing-protocol/1',reason='invalid_output',applicability='current'))
        self.assertEqual(m.plan_ablations(seal(b))['validator']['provenance'],'recorded_rejection')
        a['delivery']['rejection']['validator']='forged'
        with self.assertRaises(ValueError):m.plan_ablations(seal(b))

    def test_nested_manifest_runtime_types_and_cost_overflow_refuse(self):
        m=self.module();b=copy.deepcopy(self.actual);a=b['attempts'][0]
        manifest=json.loads(decode_envelope(a['immutable_manifest']))
        manifest['runtime']['threads']=True
        a['immutable_manifest']=envelope(canonical_bytes(manifest))
        runtime=json.loads(decode_envelope(a['immutable_runtime']))
        runtime['prompt_fingerprint']=a['immutable_manifest']['sha256']
        a['immutable_runtime']=envelope(canonical_bytes(runtime))
        timing=json.loads(decode_envelope(a['timing_configuration']));timing['runtime']=runtime
        a['timing_configuration']=envelope(canonical_bytes(timing))
        with self.subTest(case='manifest_bool'),self.assertRaises(ValueError):m.plan_ablations(seal(b))
        p=m.plan_ablations(self.actual);o=outcomes(p)
        for row in o['rows']:row.update(reserved_bound=(1<<63)-1,charged_units=(1<<63)-1)
        with self.subTest(case='aggregate_overflow'),self.assertRaises(ValueError):m.score_ablations(p,o)
        b=copy.deepcopy(self.actual);b['attempts'][0]['accounting']['unit']='logical_support'
        with self.subTest(case='source_unit'),self.assertRaises(ValueError):m.plan_ablations(seal(b))

    def test_scores_ties_denominators_and_unit_totals(self):
        m=self.module();p=m.plan_ablations(self.actual);o=outcomes(p)
        for r in o['rows'][3:9]: r['semantic_result']['value']={'decision':'route'}
        report=m.score_ablations(p,o)
        self.assertEqual(report['coverage'],'Complete');self.assertEqual(report['baseline_status'],'Stable')
        self.assertEqual([v['association_score'] for v in report['variants']],[1.0,1.0,0.0])
        self.assertEqual([v['rank'] for v in report['variants']],[1,1,3])
        self.assertEqual([v['removed_ids'] for v in report['variants']],[[2],[3],[4]])
        self.assertEqual(report['recorded_calls'],12)
        self.assertEqual(report['costs'],[dict(unit='fixture_bytes',charged_units=24,known_charged_units=24,unknown_charges=0,reserved_bound=120,held_units=0)])

    def test_unstable_failed_unknown_missing_baselines_are_inconclusive(self):
        m=self.module();p=m.plan_ablations(self.actual)
        for change in ('unstable','failed','unknown','missing'):
            o=outcomes(p)
            if change=='unstable':o['rows'][0]['semantic_result']['value']=True
            elif change=='missing':o['rows'].pop(0)
            else:o['rows'][0].update(state=change,response_digest=None,semantic_result=None,charged_units=None)
            r=m.score_ablations(p,o)
            self.assertEqual(r['baseline_status'],'Inconclusive',change)
            self.assertTrue(all(v['rank'] is None and v['association_score'] is None for v in r['variants']))
            self.assertEqual(r['coverage'],'Incomplete' if change=='missing' else 'Complete')

    def test_variant_missing_failed_unknown_no_rank(self):
        m=self.module();p=m.plan_ablations(self.actual)
        for state in ('missing','failed','unknown'):
            o=outcomes(p)
            if state=='missing':o['rows'].pop(3)
            else:o['rows'][3].update(state=state,response_digest=None,semantic_result=None,charged_units=None)
            r=m.score_ablations(p,o);v=r['variants'][0]
            self.assertIsNone(v['rank']);self.assertIsNone(v['association_score'])
            self.assertEqual(v['scheduled'],3);self.assertEqual(v['unchanged'],2)
            self.assertEqual(v['missing'] if state=='missing' else v[state],1)

    def test_unknown_cost_and_mixed_units_remain_distinct(self):
        m=self.module();p=m.plan_ablations(self.actual);o=outcomes(p)
        o['rows'][0].update(charged_units=None);o['rows'][1].update(unit='model_tokens');o['rows'][2].update(unit='unknown',charged_units=None)
        r=m.score_ablations(p,o);costs={c['unit']:c for c in r['costs']}
        self.assertIsNone(costs['fixture_bytes']['charged_units']);self.assertEqual(costs['fixture_bytes']['unknown_charges'],1)
        self.assertEqual(costs['model_tokens']['charged_units'],2);self.assertIsNone(costs['unknown']['charged_units'])
        self.assertEqual(costs['fixture_bytes']['reserved_bound'],100)

    def test_outcome_duplicates_unknown_ids_digest_and_nested_types_refuse(self):
        m=self.module();p=m.plan_ablations(self.actual)
        changes=[lambda o:o['rows'].append(copy.deepcopy(o['rows'][0])),lambda o:o['rows'][0].update(trial_id='unknown'),
                 lambda o:o.update(plan_digest='0'*64), lambda o:o['rows'][0].update(charged_units=True),
                 lambda o:o['rows'][0].update(semantic_result={'version':'arbitrary','value':None}),
                 lambda o:o['rows'][0].update(semantic_result={'version':'recorded-normalized-result/1','value':1.5}),
                 lambda o:o['rows'][0].update(response_digest=None),lambda o:o['rows'][0].update(charged_units=11)]
        for index,change in enumerate(changes):
            o=outcomes(p);change(o)
            with self.subTest(index=index),self.assertRaises(ValueError):m.score_ablations(p,o)

    def test_comparison_identity_mismatch_preserves_other_statuses(self):
        m=self.module();p=m.plan_ablations(self.actual)
        for field in ('scorer','runtime','validator'):
            o=outcomes(p)
            if field=='scorer':o['rows'][0][field]='other/1'
            elif field=='validator':o['rows'][0][field]['provenance']='unknown'
            else:o['rows'][0][field]=envelope(b'other runtime')
            o['rows'].pop()
            r=m.score_ablations(p,o)
            self.assertEqual(r['comparison_status'],'Incomparable');self.assertEqual(r['coverage'],'Incomplete')
            self.assertTrue(all(v['rank'] is None for v in r['variants']))

    def test_rehashed_malformed_plans_cannot_shrink_schedule(self):
        m=self.module();p=m.plan_ablations(rebuild(self.actual,4),include_pairs=True)
        changes=[lambda p:p['rows'].pop(0),lambda p:p['rows'].__delitem__(slice(-3,None)),
                 lambda p:p['rows'][0].update(repeat_index=1), lambda p:p['rows'][0].update(variant_digest='0'*64),
                 lambda p:p.update(scheduled_calls=1),lambda p:p.update(trials=True),
                 lambda p:p.update(selected_ids=[2,2]),lambda p:p['rows'][0].update(removed_ids=[2])]
        for index,change in enumerate(changes):
            bad=copy.deepcopy(p);change(bad)
            with self.subTest(index=index),self.assertRaises(ValueError):m.score_ablations(bad,outcomes(bad))

    def test_normalized_result_strict_type_equality(self):
        m=self.module();p=m.plan_ablations(self.actual);o=outcomes(p)
        for r in o['rows']:r['semantic_result']['value']=1
        o['rows'][3]['semantic_result']['value']=True
        self.assertEqual(m.score_ablations(p,o)['variants'][0]['changed'],1)

    def test_public_objects_and_transport_bounded_before_serialization(self):
        m=self.module()
        huge={'x':'x'*(16*1024*1024+1)}
        for func,args in ((m.plan_ablations,(huge,)),(m.score_ablations,(huge,{})),(m.score_ablations,({},huge))):
            with patch.object(m,'canonical_bytes',side_effect=AssertionError('serialized unbounded object')):
                with self.assertRaises(ValueError):func(*args)
        cycle=[];cycle.append(cycle)
        with self.assertRaises(ValueError):m.plan_ablations(cycle)
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'big'
            with path.open('wb') as stream:stream.truncate(16*1024*1024+1)
            with self.assertRaises(ValueError):m.read_json(path)
        for raw in (b'{"a":1,"a":2}',b'['*65+b'0'+b']'*65,b'{"x":1.0}'):
            with self.assertRaises(ValueError):m.parse_json(raw)

    def test_api_and_both_cli_commands_cannot_execute_process_or_network(self):
        m=self.module()
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder);(path/'bundle').write_bytes(canonical_bytes(self.actual))
            with ExitStack() as stack:
                for obj,name in ((subprocess,'Popen'),(subprocess,'run'),(os,'system'),(os,'popen'),(os,'posix_spawn'),(socket,'socket'),(socket,'create_connection'),(urllib.request,'urlopen')):
                    stack.enter_context(patch.object(obj,name,side_effect=AssertionError('execution forbidden')))
                p=m.plan_ablations(self.actual);o=outcomes(p);m.score_ablations(p,o)
                (path/'plan').write_bytes(canonical_bytes(p));(path/'outcomes').write_bytes(canonical_bytes(o))
                for args in (['plan',str(path/'bundle')],['score',str(path/'plan'),str(path/'outcomes')]):
                    output=io.StringIO()
                    with redirect_stdout(output):self.assertEqual(m.main(args),0)
                    self.assertIsInstance(json.loads(output.getvalue()),dict)
                bad=copy.deepcopy(self.actual);bad['attempts'][0]['inputs'][0]['role']='report';(path/'bad').write_bytes(canonical_bytes(seal(bad)))
                with redirect_stderr(io.StringIO()):self.assertNotEqual(m.main(['plan',str(path/'bad')]),0)
                with redirect_stderr(io.StringIO()),self.assertRaises(SystemExit):m.main(['run',str(path/'bundle')])

    def test_resealed_request_and_role_attacks_differential_rust_refuse(self):
        m=self.module()
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'mutated'
            for attack in ('request','role','source','missing_identity'):
                b=copy.deepcopy(self.actual);a=b['attempts'][0]
                if attack=='request':a['dispatch_request']=a['recovery']['request']=envelope(b'forged request')
                elif attack=='role':a['inputs'][0]['role']='report'
                elif attack=='source':b['evidence'][1]['payload']=envelope(b'forged payload')
                else:a['immutable_runtime']=None
                b=seal(b);path.write_bytes(canonical_bytes(b))
                result=subprocess.run([self.runner,'replay',str(path)],capture_output=True,timeout=30)
                if result.returncode==0:
                    self.assertNotEqual(json.loads(result.stdout)['reconstruction'],'matched',attack)
                with self.subTest(attack=attack),patch.object(subprocess,'run',side_effect=AssertionError('no validation process')):
                    with self.assertRaises(ValueError):m.plan_ablations(b)

    def test_synthetic_supported_variants_differential_rust_reconstruction(self):
        m=self.module()
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'synthetic'
            for count in (3,4,5):
                b=rebuild(self.actual,count,identical=True);path.write_bytes(canonical_bytes(b))
                result=subprocess.run([self.runner,'replay',str(path)],capture_output=True,timeout=30)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(json.loads(result.stdout)['reconstruction'],'matched')
                self.assertEqual(len(m.plan_ablations(b,selected_ids=[2])['selected_ids']),1)


if __name__=='__main__':unittest.main()
