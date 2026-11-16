"""Bounded offline hypothetical schedules and recorded-result attestations.

No backend, process, model, network or validator execution. Contract checks below
are deliberately narrower than Rust ledger validation and confer no authority.
"""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import sys

from decision_bundle_v1 import (MAX_BUNDLE_BYTES, MAX_JSON_DEPTH, DIGEST,
    canonical_bytes, decode_envelope, bounded_depth, unique_object, reject_number,
    parse_bundle, require)

SCORER = 'recorded-result-equality/1'
VALIDATOR = 'incident-routing-protocol/1'
PLAN = 'decision-ablation-plan/1'
OUTCOMES = 'decision-ablation-outcomes/1'
RESULT = 'recorded-normalized-result/1'
INCIDENT_LIMIT = 1024 * 1024
I64 = (1 << 63) - 1
RUNTIME_FIELDS = {'model', 'model_revision', 'tokenizer_revision', 'runtime_version',
    'dtype', 'decoding', 'cpu_fingerprint', 'threads', 'prompt_fingerprint', 'readiness'}
ATTEMPT_FIELDS = {'actor', 'scope', 'id', 'branch', 'captured_epoch', 'captured_revision',
    'parent', 'inherited_from', 'candidates', 'inputs', 'dispatch_request', 'delivery',
    'validator', 'immutable_runtime', 'immutable_manifest', 'timing_configuration',
    'accounting', 'reuse_origin', 'model_contract', 'deterministic', 'support', 'recovery'}
IDENTITIES = ('scorer', 'runtime', 'validator')


def integer(value, low=0, high=I64):
    require(type(value) is int and low <= value <= high, 'integer type or bound')


def text(value):
    require(type(value) is str and bool(value.strip()), 'nonempty text required')


def fields(value, keys):
    require(type(value) is dict and set(value) == set(keys), 'incorrect record fields')


def digest(value):
    require(type(value) is str and DIGEST.fullmatch(value), 'invalid digest')


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def preflight(value, limit=MAX_BUNDLE_BYTES):
    """Count exact compact UTF-8 JSON size before serialization or cloning.

    Reject arbitrary objects, cycles, depth, wide arrays/maps and oversized strings
    without allocating encoded strings/JSON intermediates. Integers are i64.
    """
    size = 0
    ancestors = set()

    def add(count):
        nonlocal size
        size += count
        require(size <= limit, 'object byte limit exceeded')

    def string(s):
        require(type(s) is str and len(s) <= limit, 'string type or byte limit')
        add(2)
        for char in s:
            n = ord(char)
            require(not 0xd800 <= n <= 0xdfff, 'invalid UTF-8 surrogate')
            add(2 if char in '\"\\\b\f\n\r\t' else 6 if n < 32 else
                1 if n < 128 else 2 if n < 2048 else 3 if n < 65536 else 4)

    def visit(v, depth):
        require(depth <= MAX_JSON_DEPTH, 'object depth limit exceeded')
        kind = type(v)
        if kind is str:
            string(v)
        elif kind is bool:
            add(4 if v else 5)
        elif v is None:
            add(4)
        elif kind is int:
            require(-I64-1 <= v <= I64, 'integer bound')
            add(len(str(v)))
        elif kind in (dict, list):
            require(id(v) not in ancestors and len(v) <= limit // 2, 'cycle or container limit')
            add(2 + max(0, len(v)-1))
            ancestors.add(id(v))
            if kind is dict:
                for k, child in v.items():
                    string(k); add(1); visit(child, depth+1)
            else:
                for child in v:
                    visit(child, depth+1)
            ancestors.remove(id(v))
        else:
            raise ValueError('unsupported JSON type')
    visit(value, 1)


def parse_json(raw, limit=MAX_BUNDLE_BYTES):
    require(type(raw) is bytes and len(raw) <= limit, 'transport byte limit exceeded')
    bounded_depth(raw)
    try:
        value = json.loads(raw.decode('utf-8'), object_pairs_hook=unique_object,
                           parse_float=reject_number, parse_constant=reject_number)
        preflight(value, limit)
        return value
    except (UnicodeError, RecursionError, OverflowError) as error:
        raise ValueError('invalid JSON encoding') from error


def read_json(path):
    with Path(path).open('rb') as stream:
        return parse_json(stream.read(MAX_BUNDLE_BYTES+1))


def _embedded(envelope, limit=None):
    raw = decode_envelope(envelope)
    return parse_json(raw, limit or MAX_BUNDLE_BYTES), raw


def _runtime(runtime):
    fields(runtime, RUNTIME_FIELDS)
    for key in RUNTIME_FIELDS - {'threads'}:
        text(runtime[key])
    integer(runtime['threads'], 1)
    require(runtime['readiness'] in ('cold', 'warm'), 'unknown readiness')
    digest(runtime['prompt_fingerprint'])


def _validator(binding, supported=True):
    fields(binding, {'version', 'provenance'})
    text(binding['version']); text(binding['provenance'])
    if supported:
        require(binding['version'] == VALIDATOR and binding['provenance'] in
                ('declared_contract', 'recorded_rejection'), 'unsupported validator identity')


def _group_positions(inputs):
    """Internal occurrence grouping; supported public incident/1 forbids repeats."""
    grouped = {}
    for row in inputs:
        integer(row['evidence_id'], 1); integer(row['position'])
        grouped.setdefault(row['evidence_id'], []).append(row['position'])
    return [dict(evidence_id=i, positions=p) for i,p in grouped.items()]


def _accounting(a):
    q = a['accounting']
    fields(q, {'unit','quote','granted','budget_reference','charge','hold_bound'})
    require(q['unit'] in ('logical_support','fixture_bytes','model_tokens','unknown'), 'accounting unit')
    integer(q['charge']); integer(q['hold_bound'])
    require(q['granted'] is None or type(q['granted']) is bool, 'grant type')
    if q['budget_reference'] is not None:
        integer(q['budget_reference'], 1)
    bound = None
    if q['quote'] is not None:
        fields(q['quote'], {'input_tokens','output_cap'})
        integer(q['quote']['input_tokens']);integer(q['quote']['output_cap'], 1)
        bound = q['quote']['input_tokens'] + q['quote']['output_cap']
        integer(bound)
        require(type(q['granted']) is bool and q['budget_reference'] is not None, 'incomplete reservation')
    else:
        require(q['granted'] is None and q['budget_reference'] is None, 'unbound reservation')
    d = a['delivery']
    require(type(d) is dict, 'delivery record')
    if d.get('state') == 'unresolved':
        fields(d, {'state'})
        require(q['charge'] == 0, 'unresolved charge')
        expected = bound if q['granted'] is True and a['recovery']['state'] != 'cancelled' else 0
        require(q['hold_bound'] == expected, 'unresolved hold mismatch')
    else:
        fields(d, {'state','raw','disposition','usage','rejection'})
        require(d['state'] == 'recorded' and d['disposition'] in
                ('accepted','stale_view','stale_branch','invalid_output'), 'unknown delivery')
        decode_envelope(d['raw']);integer(d['usage'])
        require(d['usage'] == q['charge'] and q['hold_bound'] == 0, 'recorded accounting mismatch')
        if bound is not None:
            require(q['granted'] is True and q['quote']['input_tokens'] <= d['usage'] <= bound,
                    'recorded usage outside reservation')
        if d['disposition'] == 'invalid_output':
            fields(d['rejection'], {'validator','reason','applicability'})
            text(d['rejection']['validator']);text(d['rejection']['reason'])
            require(d['rejection']['applicability'] in ('current','stale_view','stale_branch'), 'rejection applicability')
        else:
            require(d['rejection'] is None, 'unexpected rejection')
    return bound


def _applicable(bundle):
    # Public constructed objects are bounded before the reader serializes/clones.
    preflight(bundle)
    b = parse_bundle(canonical_bytes(bundle))
    require(len(b['attempts']) == 1, 'only direct initial root supported')
    integer(b['root_attempt'], 1);text(b['actor']);text(b['scope'])
    require(b['export_purpose'] in ('current_read','diagnostic_replay'), 'unknown export purpose')
    snapshot = b['snapshot']
    fields(snapshot, {'root_branch','head','revision','current_epoch'})
    integer(snapshot['root_branch'],1);integer(snapshot['revision']);integer(snapshot['current_epoch'])
    if snapshot['head'] is not None:integer(snapshot['head'],1)
    a = b['attempts'][0]
    fields(a, ATTEMPT_FIELDS)
    for key in ('id','branch'):integer(a[key],1)
    for key in ('captured_epoch','captured_revision'):integer(a[key])
    if a['parent'] is not None:integer(a['parent'],1)
    require(a['id'] == b['root_attempt'] and a['branch'] == snapshot['root_branch']
            and a['actor'] == b['actor'] and a['scope'] == b['scope'], 'root identity mismatch')
    require(all(a[k] is None for k in ('inherited_from','reuse_origin','model_contract','deterministic','support')),
            'unsupported dependency or policy contract')
    r = a['recovery']
    fields(r, {'request','recovery_id','ordinal','state','predecessor_id'})
    integer(r['recovery_id'],1);integer(r['ordinal'])
    require(r['ordinal'] == 0 and r['predecessor_id'] is None, 'retry unsupported')
    require(r['state'] in ('planned','reserved','sent','cancelled','denied'), 'unknown recovery state')
    bound = _accounting(a)
    q = a['accounting']
    if r['state'] == 'planned':
        require(q['quote'] is None and r['request'] is None, 'planned request inconsistent')
    if r['state'] in ('reserved','sent'):
        require(q['granted'] is True, 'reservation state mismatch')
    if r['state'] == 'denied':require(q['granted'] is False, 'denied reservation mismatch')
    if r['state'] == 'cancelled':require(q['granted'] is not False, 'cancelled reservation mismatch')
    if r['state'] in ('reserved','sent','denied'):require(r['request'] is not None, 'missing recovery request')
    require((a['dispatch_request'] is not None) == (r['state'] == 'sent'), 'dispatch state mismatch')
    require(a['delivery']['state'] != 'recorded' or r['state'] == 'sent', 'unsent delivery')
    require(r['request'] is not None, 'captured request unavailable')

    manifest, manifest_raw = _embedded(a['immutable_manifest'])
    fields(manifest, {'context','freeze_sha256','initial_renderer','output_cap','prompt_sha256',
        'retry_feedback','retry_renderer','runtime','schema_version','sources','validator_version','version','workload_version'})
    integer(manifest['schema_version'],1,1)
    for key, expected in dict(version='incident-store-manifest/1',workload_version='incident-routing/1',
            validator_version=VALIDATOR,initial_renderer='incident-request/1',retry_renderer='incident-request-retry/1',
            retry_feedback=VALIDATOR+':invalid_output').items():
        require(manifest[key] == expected, 'unsupported manifest contract')
    require(manifest_raw == canonical_bytes(manifest), 'manifest must be canonical')
    integer(manifest['output_cap'],1)
    digest(manifest['freeze_sha256']);digest(manifest['prompt_sha256'])
    runtime, _ = _embedded(a['immutable_runtime']);_runtime(runtime)
    timing, _ = _embedded(a['timing_configuration'])
    fields(timing, {'clock_domain','start_ms','deadline_ms','runtime','calibration','safety_margin_ms'})
    _runtime(timing['runtime']);text(timing['clock_domain'])
    for key in ('start_ms','deadline_ms','safety_margin_ms'):integer(timing[key])
    cal=timing['calibration']
    fields(cal, {'id','max_duration_ms','sample_count','max_input_tokens','output_cap','measured_at_ms','expires_at_ms'})
    text(cal['id'])
    for key in ('max_duration_ms','sample_count','output_cap'):integer(cal[key],1)
    for key in ('max_input_tokens','measured_at_ms','expires_at_ms'):integer(cal[key])
    require(timing['deadline_ms'] > timing['start_ms'] and cal['expires_at_ms'] > cal['measured_at_ms'], 'invalid timing bounds')
    require(timing['runtime'] == runtime and runtime['prompt_fingerprint'] == sha(manifest_raw)
            and cal['output_cap'] == manifest['output_cap'], 'runtime/timing/manifest mismatch')
    fields(manifest['runtime'], RUNTIME_FIELDS - {'prompt_fingerprint'})
    _runtime(dict(manifest['runtime'], prompt_fingerprint=runtime['prompt_fingerprint']))
    require(manifest['runtime'] == {k:v for k,v in runtime.items() if k != 'prompt_fingerprint'}, 'manifest runtime mismatch')
    fixture_identity = dict(model='canned-no-model', model_revision='fixture/1', tokenizer_revision='utf8-bytes/1',
        runtime_version='incident-canned/1', dtype='bytes', decoding='canned-no-inference')
    require(q['unit']=='unknown' or (q['unit']=='fixture_bytes' and
            all(runtime[k]==v for k,v in fixture_identity.items())), 'unsupported incident accounting unit')
    if r['state']=='sent':
        require(q['quote'] is not None and q['quote']['input_tokens']<=cal['max_input_tokens'], 'sent quote outside calibration')
    _validator(a['validator'])
    rejection = a['delivery'].get('rejection')
    if rejection is not None:
        require(rejection['validator'] == VALIDATOR, 'recorded rejection validator mismatch')
    if a['validator']['provenance'] == 'recorded_rejection':
        require(rejection is not None and a['delivery']['disposition'] == 'invalid_output', 'missing recorded rejection')
    else:
        require(rejection is None, 'rejection provenance mismatch')

    require(type(b['evidence']) is list and len(b['evidence']) <= 1024, 'evidence record bound')
    evidence={}
    for e in b['evidence']:
        fields(e, {'occurrence_id','source','payload','authorized_scopes'})
        integer(e['occurrence_id'],1)
        require(e['occurrence_id'] not in evidence, 'duplicate evidence occurrence')
        evidence[e['occurrence_id']]=e
        decode_envelope(e['source']);decode_envelope(e['payload'])
        scopes=e['authorized_scopes']
        require(type(scopes) is list and bool(scopes), 'missing authorized scopes')
        seen=set()
        for s in scopes:
            fields(s, {'actor','scope'});text(s['actor']);text(s['scope'])
            pair=(s['actor'],s['scope']);require(pair not in seen, 'duplicate scope');seen.add(pair)
    inputs=a['inputs'];candidates=a['candidates'];sources=manifest['sources']
    require(type(inputs) is list and 2 <= len(inputs) <= 1024 and type(candidates) is list
            and type(sources) is list and len(inputs)==len(candidates)==len(sources)+1, 'input/source alignment')
    seen=set()
    for position,(i,c) in enumerate(zip(inputs,candidates)):
        fields(i, {'position','evidence_id','role'});fields(c, {'position','evidence_id'})
        integer(i['position']);integer(c['position']);integer(i['evidence_id'],1);integer(c['evidence_id'],1)
        require(i['position'] == c['position'] == position and i['evidence_id'] == c['evidence_id'], 'candidate alignment')
        require(i['evidence_id'] not in seen, 'incident/1 duplicate occurrence');seen.add(i['evidence_id'])
        require(i['role'] == ('context' if position==0 else 'report'), 'role differs from manifest-derived role')
        require(i['evidence_id'] in evidence and dict(actor=a['actor'],scope=a['scope']) in
                evidence[i['evidence_id']]['authorized_scopes'], 'missing or unauthorized input')
    fields(manifest['context'], {'occurrence_id','sha256'})
    integer(manifest['context']['occurrence_id'],1);digest(manifest['context']['sha256'])
    first=evidence[inputs[0]['evidence_id']]
    require(manifest['context']['occurrence_id'] == first['occurrence_id'] and
            manifest['context']['sha256'] == first['payload']['sha256'], 'context identity mismatch')
    context,_ = _embedded(first['payload'], INCIDENT_LIMIT)
    fields(context, {'schema_version','version','workload_version','freeze_sha256','prompt_sha256',
        'validator_version','case_id','question','owner_ids','evidence_ids','system_prompt'})
    integer(context['schema_version'],1,1)
    require(context['version']=='incident-store-context/1', 'unsupported context')
    for key in ('workload_version','freeze_sha256','prompt_sha256','validator_version'):
        require(context[key] == manifest[key], 'context manifest mismatch')
    for key in ('case_id','question','system_prompt'):text(context[key])
    for key in ('owner_ids','evidence_ids'):
        ids=context[key];require(type(ids) is list and bool(ids), 'empty context IDs')
        for i in ids:text(i)
        require(len(set(ids))==len(ids), 'duplicate external ID')
    require(len(context['evidence_ids']) == len(sources), 'context source count')
    preflight(context, INCIDENT_LIMIT)
    reports=[]
    for i,s,external in zip(inputs[1:], sources, context['evidence_ids']):
        fields(s, {'case_id','external_id','occurrence_id','payload_sha256','workload_version'})
        integer(s['occurrence_id'],1);digest(s['payload_sha256'])
        e=evidence[i['evidence_id']]
        require(s['occurrence_id']==i['evidence_id'] and s['case_id']==context['case_id']
                and s['external_id']==external and s['workload_version']==manifest['workload_version']
                and s['payload_sha256']==e['payload']['sha256'], 'scoped source mapping mismatch')
        try: report=decode_envelope(e['payload']).decode('utf-8')
        except UnicodeError as error: raise ValueError('invalid report UTF-8') from error
        text(report);reports.append(dict(id=external,text=report))
    user=dict(question=context['question'],owner_ids=context['owner_ids'],evidence=reports)
    preflight(user, INCIDENT_LIMIT)
    request=dict(messages=[dict(role='system',content=context['system_prompt']),
        dict(role='user',content=canonical_bytes(user).decode('utf-8'))])
    preflight(request, INCIDENT_LIMIT);request_raw=canonical_bytes(request)
    require(decode_envelope(r['request']) == request_raw, 'reconstructed recovery request mismatch')
    if a['dispatch_request'] is not None:
        require(decode_envelope(a['dispatch_request']) == request_raw, 'reconstructed dispatch request mismatch')
    if bound is not None:
        require(q['quote']['output_cap']==manifest['output_cap'], 'quote output cap mismatch')
    return b, a


def _bounds(trials, include_pairs, max_calls):
    integer(trials,1,100);integer(max_calls,1,10000)
    require(type(include_pairs) is bool, 'include_pairs must be boolean')


def _variants(ids, include_pairs):
    return [[]] + [[i] for i in ids] + (sorted(map(list,itertools.combinations(ids,2))) if include_pairs else [])


def _schedule(source, ids, trials, pairs, max_calls, identities):
    variants=_variants(ids,pairs)
    count=len(variants)*trials
    require(count <= max_calls, 'scheduled calls exceed max_calls')
    rows=[]
    for removed in variants:
        variant=sha(canonical_bytes(dict(version='decision-ablation-variant/1',source_bundle_digest=source,
            removed_ids=removed,**identities)))
        for repeat in range(trials):
            rows.append(dict(trial_id=f'{variant}:{repeat}',repeat_index=repeat,removed_ids=removed[:],variant_digest=variant))
    return rows


def plan_ablations(bundle, trials=3, include_pairs=False, max_calls=33, *, selected_ids=None):
    """Validate declared initial incident/1 applicability and emit a bounded plan."""
    _bounds(trials,include_pairs,max_calls)
    if selected_ids is not None:
        require(type(selected_ids) is list and len(selected_ids)<=4, 'select at most four IDs explicitly')
        for i in selected_ids:integer(i,1)
        require(len(set(selected_ids))==len(selected_ids), 'duplicate selection')
    b,a=_applicable(bundle)
    selectable=_group_positions(a['inputs'][1:])
    if selected_ids is None:
        require(len(selectable)<=4, 'more than four items require explicit selection')
        selected_ids=[i['evidence_id'] for i in selectable]
    all_ids={i['evidence_id'] for i in selectable}
    require(all(i in all_ids for i in selected_ids), 'unknown or unselectable ID')
    positions=[i for i in selectable if i['evidence_id'] in selected_ids]
    ids=[i['evidence_id'] for i in positions]
    source=sha(canonical_bytes(b))
    identities=dict(scorer=SCORER,runtime=a['immutable_runtime'],validator=a['validator'])
    rows=_schedule(source,ids,trials,include_pairs,max_calls,identities)
    result=dict(version=PLAN,source_bundle_digest=source,selected_ids=ids,selected_positions=positions,
        trials=trials,include_pairs=include_pairs,max_calls=max_calls,scheduled_calls=len(rows),rows=rows,**identities)
    preflight(result)
    return result


def _checked_plan(plan):
    preflight(plan)
    fields(plan, {'version','source_bundle_digest','selected_ids','selected_positions','trials','include_pairs',
        'max_calls','scheduled_calls','rows',*IDENTITIES})
    require(plan['version']==PLAN and plan['scorer']==SCORER, 'unsupported plan/scorer')
    digest(plan['source_bundle_digest']);_bounds(plan['trials'],plan['include_pairs'],plan['max_calls'])
    runtime,_=_embedded(plan['runtime']);_runtime(runtime);_validator(plan['validator'])
    ids=plan['selected_ids'];positions=plan['selected_positions']
    require(type(ids) is list and len(ids)<=4 and type(positions) is list and len(positions)==len(ids), 'selected ID shape')
    for i in ids:integer(i,1)
    require(len(set(ids))==len(ids), 'duplicate selected ID')
    last=0
    for i,p in zip(ids,positions):
        fields(p, {'evidence_id','positions'});integer(p['evidence_id'],1)
        require(p['evidence_id']==i and type(p['positions']) is list and len(p['positions'])==1,
                'incident/1 position identity')
        integer(p['positions'][0],1,1023)
        require(p['positions'][0]>last, 'selected positions must follow original order');last=p['positions'][0]
    integer(plan['scheduled_calls'],1,10000)
    identities={k:plan[k] for k in IDENTITIES}
    expected=_schedule(plan['source_bundle_digest'],ids,plan['trials'],plan['include_pairs'],plan['max_calls'],identities)
    require(type(plan['rows']) is list and len(plan['rows'])==len(expected), 'incomplete plan schedule')
    # Canonical comparison also refuses bool-as-int in nested schedule rows.
    require(plan['scheduled_calls']==len(expected) and canonical_bytes(plan['rows'])==canonical_bytes(expected), 'inconsistent plan schedule')
    return expected


def _result(value):
    fields(value, {'version','value'})
    require(value['version']==RESULT, 'unsupported normalized-result encoding')
    preflight(value, INCIDENT_LIMIT)
    return canonical_bytes(value)


def score_ablations(plan, outcomes):
    """Compare producer-supplied attestations after independently checking schedule.

    Without the source bundle this checks plan consistency, not applicability.
    """
    preflight(plan);preflight(outcomes)
    rows=_checked_plan(plan)
    fields(outcomes, {'version','plan_digest','rows'})
    require(outcomes['version']==OUTCOMES, 'unsupported outcomes')
    digest(outcomes['plan_digest'])
    require(outcomes['plan_digest']==sha(canonical_bytes(plan)), 'changed plan digest')
    require(type(outcomes['rows']) is list and len(outcomes['rows'])<=len(rows), 'outcome count bound')
    scheduled={r['trial_id']:r for r in rows};observed={};mismatches=[];costs={}
    for o in outcomes['rows']:
        fields(o, {'trial_id','state','response_digest','semantic_result','charged_units','reserved_bound','unit',*IDENTITIES})
        require(type(o['trial_id']) is str and o['trial_id'] in scheduled and o['trial_id'] not in observed, 'unknown or duplicate trial ID')
        require(o['state'] in ('recorded','failed','unknown'), 'unknown outcome state')
        if o['response_digest'] is not None:digest(o['response_digest'])
        if o['state']=='recorded':
            digest(o['response_digest']);_result(o['semantic_result'])
        else:
            require(o['semantic_result'] is None, 'unrecorded semantic result')
        integer(o['reserved_bound'])
        if o['charged_units'] is not None:
            integer(o['charged_units']);require(o['charged_units']<=o['reserved_bound'], 'charge exceeds bound')
        require(o['unit'] in ('logical_support','fixture_bytes','model_tokens','unknown'), 'unknown unit label')
        text(o['scorer']);decode_envelope(o['runtime']);_validator(o['validator'],False)
        for key in IDENTITIES:
            if canonical_bytes(o[key]) != canonical_bytes(plan[key]):
                mismatches.append(dict(trial_id=o['trial_id'],identity=key))
        observed[o['trial_id']]=o
        held = o['reserved_bound'] if o['charged_units'] is None or o['state']!='recorded' else 0
        if o['unit']=='unknown':
            # Equal unspecified labels do not establish a shared measurement unit.
            c=costs.setdefault('unknown',dict(unit='unknown',charged_units=None,known_charged_units=None,
                unknown_charges=0,reserved_bound=None,held_units=None,observed_trials=0,observations=[]))
            c['observed_trials']+=1
            c['unknown_charges']+=o['charged_units'] is None
            c['observations'].append(dict(trial_id=o['trial_id'],state=o['state'],
                charged_units=o['charged_units'],reserved_bound=o['reserved_bound'],held_units=held))
        else:
            c=costs.setdefault(o['unit'],dict(unit=o['unit'],charged_units=0,known_charged_units=0,unknown_charges=0,reserved_bound=0,held_units=0))
            if o['charged_units'] is None:
                c['unknown_charges']+=1
            else:c['known_charged_units']+=o['charged_units']
            c['reserved_bound']+=o['reserved_bound']
            c['held_units']+=held
    for c in costs.values():
        if c['unit']=='unknown':continue
        for key in ('known_charged_units','reserved_bound','held_units'):integer(c[key])
        integer(c['known_charged_units']+c['held_units'])
        c['charged_units']=None if c['unknown_charges'] else c['known_charged_units']
    baseline=[observed.get(r['trial_id']) for r in rows if not r['removed_ids']]
    baseline_reasons=[]
    if any(o is None or o['state']!='recorded' for o in baseline):baseline_reasons.append('baseline_unavailable')
    baseline_values=[_result(o['semantic_result']) for o in baseline if o is not None and o['state']=='recorded']
    if len(set(baseline_values))>1:baseline_reasons.append('baseline_unstable')
    usable=not baseline_reasons and not mismatches
    variants=[]
    for removed in _variants(plan['selected_ids'],plan['include_pairs'])[1:]:
        v=dict(removed_ids=removed,scheduled=plan['trials'],changed=0,unchanged=0,failed=0,unknown=0,missing=0,
               unscored=0,association_score=None,rank=None)
        for r in rows:
            if r['removed_ids']!=removed:continue
            o=observed.get(r['trial_id'])
            if o is None:v['missing']+=1
            elif o['state']!='recorded':v[o['state']]+=1
            elif not usable:v['unscored']+=1
            else:v['unchanged' if _result(o['semantic_result'])==baseline_values[0] else 'changed']+=1
        if usable and not (v['failed'] or v['unknown'] or v['missing']):v['association_score']=v['changed']/plan['trials']
        variants.append(v)
    # Preserve source-order display, assign equal exact numerator scores equal ranks.
    ranked=[v for v in variants if v['association_score'] is not None]
    for v in ranked:v['rank']=1+sum(other['changed']>v['changed'] for other in ranked)
    missing=[r['trial_id'] for r in rows if r['trial_id'] not in observed]
    return dict(version='decision-ablation-report/1',plan_digest=outcomes['plan_digest'],
        coverage='Incomplete' if missing else 'Complete',baseline_status='Inconclusive' if baseline_reasons else 'Stable',
        baseline_reasons=baseline_reasons,comparison_status='Incomparable' if mismatches else 'Comparable',
        identity_mismatches=mismatches,scheduled_calls=len(rows),observed_outcomes=len(observed),
        recorded_calls=sum(o['state']=='recorded' for o in observed.values()),missing_trials=missing,
        costs=[costs[k] for k in sorted(costs)],unreported_cost_trials=len(missing),variants=variants,
        interpretation='Recorded attestations only; association is not causal influence or scientific truth. Execution and current authorization are not authenticated.')


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    sub=parser.add_subparsers(dest='command',required=True)
    p=sub.add_parser('plan');p.add_argument('bundle');p.add_argument('--trials',type=int,default=3)
    p.add_argument('--include-pairs',action='store_true');p.add_argument('--max-calls',type=int,default=33)
    p.add_argument('--selected-id',type=int,action='append',dest='selected_ids')
    s=sub.add_parser('score');s.add_argument('plan');s.add_argument('outcomes')
    args=parser.parse_args(argv)
    try:
        if args.command=='plan':
            result=plan_ablations(read_json(args.bundle),args.trials,args.include_pairs,args.max_calls,selected_ids=args.selected_ids)
        else:result=score_ablations(read_json(args.plan),read_json(args.outcomes))
        print(canonical_bytes(result).decode('utf-8'));return 0
    except (ValueError,OSError) as error:
        print(f'decision ablation refused: {error}',file=sys.stderr);return 1


if __name__=='__main__':sys.exit(main())
