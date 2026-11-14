"""Offline incident-routing preparation and audit. No inference/Store authorization.

Protocol validity is intentionally NOT semantic correctness. Gold labels are used
only by offline audit. All evidence eligibility is an explicit fixture/replay
attestation, not independently authenticated current Store state.
"""
import argparse
import hashlib
import json
from pathlib import Path

VERSION = 'incident-routing/1'
ROOT = Path(__file__).resolve().parents[1] / 'workloads' / 'incident-routing-v1'
FREEZE_SHA256 = 'f71732c193ff889f24709a11855b59820a657c21c2b43ec721cddfbff1ede2c8'
FILES = {'prompt.json', 'cases.json', 'labels.json', 'contract.json'}
REASONS = {'diagnosed_component', 'undetermined_cause', 'conflicting_diagnoses', 'owner_not_unique'}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def fields(value, keys, name):
    require(type(value) is dict and set(value) == set(keys), f'{name}: incorrect fields')


def text(value, name):
    require(type(value) is str and bool(value.strip()), f'{name}: nonempty string required')
    value.encode('utf-8')


def versioned(value, extra, name):
    fields(value, {'schema_version', 'version'} | set(extra), name)
    require(type(value['schema_version']) is int and value['schema_version'] == 1, 'unsupported schema')
    require(value['version'] == VERSION, 'workload version mismatch')


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False)


def digest(value):
    return hashlib.sha256(canonical(value).encode('utf-8')).hexdigest()


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON key')
        result[key] = value
    return result


def _loads(raw):
    def invalid_constant(_):
        raise ValueError('nonfinite JSON constant')
    try:
        return json.loads(raw, object_pairs_hook=_pairs, parse_constant=invalid_constant)
    except RecursionError as error:
        raise ValueError('JSON nesting exceeds parser limit') from error


def read_json(path):
    return _loads(Path(path).read_text(encoding='utf-8'))


def _inputs(workload):
    versioned(workload['prompt'], {'system'}, 'prompt')
    text(workload['prompt']['system'], 'system prompt')
    versioned(workload['cases'], {'cases'}, 'cases')
    rows = workload['cases']['cases']
    require(type(rows) is list and rows, 'empty cases')
    ids, families, evidence_splits = set(), {}, {}
    for case in rows:
        fields(case, {'case_id', 'split', 'family_id', 'question', 'owner_ids', 'evidence'}, 'case')
        for key in ('case_id', 'family_id', 'question'):
            text(case[key], key)
        require(case['case_id'] not in ids, 'duplicate case ID')
        ids.add(case['case_id'])
        require(case['split'] in ('calibration', 'heldout'), 'invalid split')
        require(families.setdefault(case['family_id'], case['split']) == case['split'], 'family split leakage')
        owners = case['owner_ids']
        require(type(owners) is list and owners, 'empty owner roster')
        for owner in owners:
            text(owner, 'owner')
        require(len(set(owners)) == len(owners), 'duplicate owner')
        require(type(case['evidence']) is list and case['evidence'], 'empty evidence')
        seen = set()
        for evidence in case['evidence']:
            fields(evidence, {'id', 'text'}, 'evidence')
            text(evidence['id'], 'evidence ID'); text(evidence['text'], 'evidence text')
            require(evidence['id'] not in seen, 'duplicate evidence ID')
            seen.add(evidence['id'])
            normalized = ' '.join(evidence['text'].lower().split())
            require(evidence_splits.setdefault(normalized, case['split']) == case['split'], 'evidence split leakage')
    require({r['split'] for r in rows} == {'calibration', 'heldout'}, 'both splits required')
    return rows


def validate_output(raw, case):
    """Format/ID validation only: never reads gold or infers semantic correctness."""
    try:
        require(type(raw) is str, 'output must be a JSON string')
        raw.encode('utf-8')
        value = _loads(raw)
        fields(value, {'decision', 'owner', 'reason', 'evidence_ids'}, 'output')
        require(type(value['decision']) is str and value['decision'] in ('route', 'abstain'), 'decision')
        require(type(value['reason']) is str and value['reason'] in REASONS, 'reason')
        if value['decision'] == 'route':
            require(type(value['owner']) is str and value['owner'] in case['owner_ids'], 'owner')
            require(value['reason'] == 'diagnosed_component', 'route reason')
        else:
            require(value['owner'] is None and value['reason'] != 'diagnosed_component', 'abstention contract')
        citations = value['evidence_ids']
        require(type(citations) is list and citations, 'citations required')
        require(all(type(c) is str for c in citations), 'citation strings required')
        require(len(set(citations)) == len(citations), 'duplicate citations')
        require(set(citations) <= {e['id'] for e in case['evidence']}, 'unknown citation')
        return value
    except (ValueError, TypeError, UnicodeError):
        return None


def load_workload(root=ROOT):
    root = Path(root)
    raw = (root / 'freeze.json').read_bytes()
    require(hashlib.sha256(raw).hexdigest() == FREEZE_SHA256, 'frozen manifest digest mismatch')
    freeze = _loads(raw)
    versioned(freeze, {'mode', 'calibration_cases', 'heldout_cases', 'new_model_calls', 'construction', 'sha256'}, 'freeze')
    fields(freeze['sha256'], FILES, 'frozen file set')
    workload = {'freeze': freeze}
    for name in sorted(FILES):
        content = (root / name).read_bytes()
        require(hashlib.sha256(content).hexdigest() == freeze['sha256'][name], f'{name}: digest mismatch')
        workload[name[:-5]] = _loads(content)
    rows = _inputs(workload)
    for split in ('calibration', 'heldout'):
        require(sum(r['split'] == split for r in rows) == freeze[f'{split}_cases'], 'frozen case count mismatch')
    versioned(workload['labels'], {'labels'}, 'labels')
    labels = workload['labels']['labels']
    require(type(labels) is list and len(labels) == len(rows), 'label coverage')
    cases = {r['case_id']: r for r in rows}; seen = set()
    for label in labels:
        fields(label, {'case_id', 'expected'}, 'label')
        require(label['case_id'] in cases and label['case_id'] not in seen, 'duplicate or unknown label ID')
        seen.add(label['case_id'])
        require(validate_output(canonical(label['expected']), cases[label['case_id']]) is not None, 'invalid reference contract')
    require(workload['contract']['version'] == VERSION and workload['contract']['reasons'] == sorted(REASONS, key=lambda x: ['diagnosed_component','undetermined_cause','conflicting_diagnoses','owner_not_unique'].index(x)), 'scoring contract mismatch')
    return workload


def prepare(workload, split='calibration', states=None):
    """Render by allowlist. Gold labels never enter any request or eligibility gate."""
    require(split in ('calibration', 'heldout', 'all'), 'unknown split')
    cases = [c for c in _inputs(workload) if split == 'all' or c['split'] == split]
    expected = {c['case_id']: {e['id'] for e in c['evidence']} for c in cases}
    if states is None:
        states = [{'case_id': c['case_id'], 'evidence': [{'id': e['id'], 'state': 'current'} for e in c['evidence']]} for c in cases]
    require(type(states) is list and len(states) == len(cases), 'complete selected evidence state required')
    state_map = {}
    for state in states:
        fields(state, {'case_id', 'evidence'}, 'case state')
        case_id = state['case_id']
        require(type(case_id) is str and case_id in expected and case_id not in state_map, 'state case ID')
        require(type(state['evidence']) is list, 'state evidence list')
        mapped = {}
        for entry in state['evidence']:
            fields(entry, {'id', 'state'}, 'evidence state')
            require(type(entry['id']) is str and entry['id'] not in mapped, 'duplicate state evidence')
            require(type(entry['state']) is str and entry['state'] in ('current', 'missing', 'stale', 'refused'), 'unknown eligibility state')
            mapped[entry['id']] = entry['state']
        require(set(mapped) == expected[case_id], 'incomplete or unknown selected evidence')
        state_map[case_id] = mapped
    records = []
    for case in cases:
        current = all(state_map[case['case_id']][e['id']] == 'current' for e in case['evidence'])
        # Family, split, case ID, annotation, labels and status never go to the model.
        payload = {key: case[key] for key in ('question', 'owner_ids', 'evidence')}
        request = canonical({'messages': [{'role': 'system', 'content': workload['prompt']['system']}, {'role': 'user', 'content': canonical(payload)}]}) if current else None
        records.append({'case_id': case['case_id'], 'request': request,
                        'request_sha256': hashlib.sha256(request.encode()).hexdigest() if request is not None else None,
                        'evidence_sha256': digest(case['evidence']),
                        'refusal': None if current else 'evidence_unavailable'})
    return {'schema_version': 1, 'version': VERSION, 'mode': 'offline_preparation', 'new_model_calls': 0,
            'split': split, 'freeze_sha256': FREEZE_SHA256, 'frozen_files': workload['freeze']['sha256'],
            'eligibility_authority': 'declared_fixture_or_replay_only_not_store_authentication',
            'states': states, 'requests': records}


def audit(workload, prepared, responses):
    """Reject corrupt/incomplete bundles; malformed model replies score incorrect."""
    result = {'schema_version': 1, 'version': VERSION, 'scored': False, 'summary': None, 'rows': [],
              'model_efficacy_claim': False, 'reason': None,
              'eligibility_authority': 'declared_fixture_or_replay_only_not_store_authentication'}
    try:
        # Re-read the pinned files rather than trusting a caller-mutated dict or
        # digests stored beside that same mutable object. Preparation remains
        # label-independent; scoring must use exactly the frozen annotations.
        require(canonical(workload) == canonical(load_workload()), 'workload differs from frozen scoring content')
        require(type(prepared) is dict, 'prepared object required')
        rebuilt = prepare(workload, prepared['split'], prepared['states'])
        require(canonical(rebuilt) == canonical(prepared), 'prepared request/provenance mismatch')
        versioned(responses, {'mode', 'prepared_sha256', 'complete', 'responses'}, 'response bundle')
        require(responses['mode'] in ('synthetic_response_fixture', 'recorded_response_replay'), 'response provenance mode')
        result['mode'] = responses['mode']
        require(responses['prepared_sha256'] == digest(prepared), 'prepared digest mismatch')
        require(type(responses['complete']) is bool, 'complete must be boolean')
        require(type(responses['responses']) is list, 'response list required')
        plans = {r['case_id']: r for r in prepared['requests']}; records = {}
        unknown = False
        for response in responses['responses']:
            fields(response, {'case_id', 'request_sha256', 'state', 'receipt_eligibility', 'output'}, 'response')
            case_id = response['case_id']
            require(type(case_id) is str and case_id in plans and case_id not in records, 'unknown or duplicate response ID')
            plan = plans[case_id]
            require(response['request_sha256'] == plan['request_sha256'], 'request digest mismatch')
            if plan['request'] is None:
                require(response['state'] == 'not_requested' and response['receipt_eligibility'] is None and response['output'] is None, 'unavailable evidence cannot have a response')
            elif response['state'] == 'unknown':
                require(response['receipt_eligibility'] is None and response['output'] is None, 'unknown response contains invented result')
                unknown = True
            else:
                require(response['state'] == 'received' and type(response['output']) is str, 'known response required')
                require(response['receipt_eligibility'] in ('current', 'stale', 'refused'), 'receipt eligibility required')
            records[case_id] = response
        require(responses['complete'] and set(records) == set(plans) and not unknown, 'incomplete_or_unknown_bundle')
        cases = {c['case_id']: c for c in workload['cases']['cases']}
        gold = {r['case_id']: r['expected'] for r in workload['labels']['labels']}
        rows = []
        for case_id, plan in plans.items():
            response = records[case_id]
            if plan['request'] is None:
                rows.append({'case_id': case_id, 'split': cases[case_id]['split'], 'not_requested': True, 'receipt_eligibility': None, 'protocol_valid': None,
                             'semantic_exact': None, 'citation_set_exact': None, 'joint_exact': None,
                             'current_joint_success': False})
                continue
            answer = validate_output(response['output'], cases[case_id]); expected = gold[case_id]
            semantic = answer is not None and all(answer[key] == expected[key] for key in ('decision', 'owner', 'reason'))
            citations = answer is not None and set(answer['evidence_ids']) == set(expected['evidence_ids'])
            rows.append({'case_id': case_id, 'split': cases[case_id]['split'], 'not_requested': False, 'receipt_eligibility': response['receipt_eligibility'], 'protocol_valid': answer is not None,
                         'semantic_exact': semantic, 'citation_set_exact': citations, 'joint_exact': semantic and citations,
                         'current_joint_success': semantic and citations and response['receipt_eligibility'] == 'current'})
        def counts(selected):
            return {'selected_cases': len(selected),
                    'not_requested': sum(r['not_requested'] for r in selected),
                    'received': sum(not r['not_requested'] for r in selected),
                    'current_received': sum(r['receipt_eligibility'] == 'current' for r in selected),
                    **{key: sum(r[key] is True for r in selected) for key in ('protocol_valid', 'semantic_exact', 'citation_set_exact', 'joint_exact', 'current_joint_success')}}
        summary = counts(rows)
        summary['by_split'] = {split: counts([r for r in rows if r['split'] == split])
                               for split in ('calibration', 'heldout') if any(r['split'] == split for r in rows)}
        result.update(scored=True, rows=rows, summary=summary)
    except (ValueError, KeyError, TypeError, UnicodeError, RecursionError, OSError) as error:
        result['reason'] = str(error)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    prep = sub.add_parser('prepare'); prep.add_argument('--out', required=True, type=Path)
    prep.add_argument('--split', choices=('calibration', 'heldout', 'all'), default='calibration')
    prep.add_argument('--states', type=Path)
    check = sub.add_parser('audit'); check.add_argument('--prepared', type=Path, required=True)
    check.add_argument('--responses', type=Path, required=True); check.add_argument('--out', type=Path)
    args = parser.parse_args(argv)
    workload = load_workload()
    if args.command == 'prepare':
        prepared = prepare(workload, args.split, read_json(args.states) if args.states else None)
        args.out.mkdir()  # Never overwrite a previous request artifact.
        (args.out / 'prepared.json').write_text(canonical(prepared) + '\n', encoding='utf-8')
        with (args.out / 'requests.jsonl').open('x', encoding='utf-8') as handle:
            for request in prepared['requests']:
                if request['request'] is not None:
                    handle.write(canonical({key: request[key] for key in ('case_id', 'request_sha256', 'request')}) + '\n')
        print(canonical({'mode': 'offline_preparation', 'selected_cases': len(prepared['requests']), 'new_model_calls': 0}))
    else:
        result = audit(workload, read_json(args.prepared), read_json(args.responses))
        if args.out:
            with args.out.open('x', encoding='utf-8') as handle:
                handle.write(canonical(result) + '\n')
        print(canonical(result))
        return 0 if result['scored'] else 2
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError, UnicodeError) as error:
        raise SystemExit(f'Offline workload refused: {error}')
