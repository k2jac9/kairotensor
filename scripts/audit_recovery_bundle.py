"""Read-only audit of a complete Store-backed recovery bundle; never runs inference.

Usage: python scripts/audit_recovery_bundle.py EXTRACTED_DIRECTORY
Requires trace.json, trace.json.raw.json and trace.json.sqlite. Raw-only calibration,
partial collections and unresolved deliveries are explicitly not scored. Files are
never modified. This verifies internal consistency, not authenticity of model or
hardware attestations. SQLite is opened mode=ro, with query_only enabled.
"""
import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3

from recovery_policy_eval import evaluate, require, unique_object

VALIDATOR = 'independent-report-verdict/1'


def parse_json(text):
    return json.loads(text, object_pairs_hook=unique_object,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError('nonfinite JSON value')))


def read_json(path):
    try:
        return parse_json(path.read_text())
    except (OSError, ValueError) as error:
        raise ValueError(f'cannot read valid {path.name}: {error}') from error


def validate_output(text, reference):
    try:
        value = parse_json(text)
    except (ValueError, UnicodeError):
        return False
    return (reference is not None and type(value) is dict
            and set(value) == {'release', 'tests', 'status'}
            and all(type(item) is str for item in value.values()) and value == reference)


def reference_for(reports):
    if len(reports) != 1:
        return None
    report = reports[0]
    if (type(report) is not dict or set(report) != {'release', 'passed', 'total'}
            or type(report['release']) is not str or not report['release']
            or type(report['passed']) is not int or type(report['total']) is not int
            or not 0 <= report['passed'] <= report['total'] or report['total'] <= 0):
        return None
    return dict(release=report['release'], tests=f"{report['passed']}/{report['total']}",
                status='pass' if report['passed'] == report['total'] else 'fail')


def checked(condition, label):
    require(condition, f'bundle mismatch: {label}')


def validate_request_envelope(request, manifest, cap, live):
    """Compare exact adapter settings, not an assertion that a model executed."""
    if live:
        checked(set(request) == {'adapter', 'model', 'revision', 'dtype', 'add_generation_prompt', 'generation', 'messages'}, 'live envelope fields')
        checked(request['adapter'] == 'transformers-cpu/v1' and request['model'] == manifest['model']
                and request['revision'] == manifest['model_revision'] and request['dtype'] == 'bfloat16'
                and request['add_generation_prompt'] is True, 'live envelope identity')
        expected = dict(do_sample=False, num_beams=1, max_new_tokens=cap)
        generation = request['generation']
        checked(type(generation) is dict and generation == expected and generation['do_sample'] is False
                and type(generation['num_beams']) is int and type(generation['max_new_tokens']) is int,
                'live request generation')
        checked(manifest['generation'] == expected | {'dtype': 'bfloat16'}
                and manifest['generation']['do_sample'] is False
                and type(manifest['generation']['num_beams']) is int
                and type(manifest['generation']['max_new_tokens']) is int, 'manifest generation')
    else:
        checked(set(request) == {'adapter', 'messages', 'output_cap'} and request['adapter'] == 'recovery-byte-token-fixture/1'
                and type(request['output_cap']) is int and request['output_cap'] == cap, 'fixture envelope')
        checked(manifest['generation'] is None, 'fixture generation')


def audit_complete(db, raw, trace):
    report = evaluate(trace)  # Retain all strict schema/pairing/split checks.
    live = raw['mode'] == 'live_qwen'
    checked(raw['mode'] in ('live_qwen', 'deterministic_byte_token_fixture'), 'collection mode')
    checked(trace['mode'] == ('recorded_policy_replay' if live else 'synthetic_policy_fixture'), 'replay mode')
    checked(raw['artifact_version'] == 1, 'raw artifact version')
    checked(raw['completed_cases'] == trace['cases'], 'completed cases')
    manifest = raw['manifest']
    corpus = manifest['corpus']
    checked(parse_json(manifest['corpus_utf8']) == corpus, 'corpus snapshot')
    checked(manifest['validator_version'] == corpus['validator_version'] == VALIDATOR, 'validator version')
    checked(corpus['corpus_id'] in trace['provenance'], 'corpus provenance')
    checked(type(corpus['output_cap']) is int and corpus['output_cap'] == 96, 'frozen corpus cap')
    if live:
        checked(manifest['model'] == 'Qwen/Qwen2.5-1.5B-Instruct', 'model identity')
        checked(manifest['model_revision'] == manifest['tokenizer_revision'] == '989aa7980e4cf806f80c7fef2b1adb7bc71aa306', 'pinned revisions')
    else:
        checked(manifest['model'] is None and manifest['model_revision'] is None and manifest['tokenizer_revision'] is None, 'fixture identity')
    definitions = {c['case_id']: c for c in corpus['cases']}
    checked(len(definitions) == len(corpus['cases']), 'duplicate corpus cases')
    checked(set(definitions) == {c['case_id'] for c in trace['cases']}, 'corpus case coverage')
    events = raw['events']
    checked(all(e['case_id'] in definitions for e in events), 'event case coverage')
    received = [e for e in events if e['phase'] == 'received']
    checked(type(raw['actual_backend_calls']) is int and raw['actual_backend_calls'] == len(received), 'backend call count')
    checked(type(raw['new_model_calls']) is int and raw['new_model_calls'] == (len(received) if live else 0), 'model call count')
    checked(not db.execute('PRAGMA foreign_key_check').fetchall(), 'foreign keys')
    all_dispatches = {r[0] for r in db.execute('SELECT attempt_id FROM dispatches')}
    checked(len(all_dispatches) == len(received), 'dispatch/receipt count')
    seen = set()
    known_total = 0
    for case in trace['cases']:
        definition = definitions[case['case_id']]
        checked((case['family'], case['split']) == (definition['family'], definition['split']), 'case split')
        checked(case['budget_tokens'] == corpus['budget_tokens'] and case['deadline_ms'] == corpus['deadline_ms'], 'case limits')
        case_events = [e for e in events if e['case_id'] == case['case_id']]
        checked(case_events and case_events[0]['phase'] == 'case_started' and case_events[-1]['phase'] == 'case_completed', 'case boundaries')
        start = case_events[0]
        binding = db.execute('SELECT budget_id,recovery_key FROM recovery_runs WHERE id=?', (start['recovery_id'],)).fetchone()
        checked(binding is not None and binding['budget_id'] == start['budget_id'] and binding['recovery_key'] == corpus['corpus_id'] + '/' + case['case_id'], 'recovery binding')
        limit_row = db.execute('SELECT token_limit FROM token_budgets WHERE id=?', (start['budget_id'],)).fetchone()
        checked(limit_row is not None and limit_row[0] == case['budget_tokens'], 'database budget limit')
        cells = [(case['initial_context'], case['initial'])]
        retry = case['retries']['affordable_retry']
        if retry is not None:
            cells.append((retry['context'], retry['attempt']))
        spent, held = 0, 0
        prepared, phases, observed = {}, {}, []
        previous_receipt_ns = 0
        last_operation_ns = 0
        received_admissions = {}
        for event in case_events:
            phase = event['phase']
            aid = event.get('attempt_id')
            if 'operation_elapsed_ns' in event:
                elapsed = event['operation_elapsed_ns']
                checked(type(elapsed) is int and elapsed >= last_operation_ns, 'event timing order')
                last_operation_ns = elapsed
            if aid is not None:
                phases.setdefault(aid, []).append(phase)
            if phase == 'prepared':
                checked(aid not in prepared, 'duplicate preparation')
                prepared[aid] = event['context']
                captured = db.execute('''SELECT e.id,e.payload FROM attempt_inputs i
                    JOIN evidence e ON e.id=i.evidence_id WHERE i.attempt_id=? ORDER BY i.position''', (aid,)).fetchall()
                checked(event['evidence'] == [dict(id=r['id'], payload_utf8=r['payload'].decode()) for r in captured], 'captured evidence')
                checked(event['context']['output_cap'] == corpus['output_cap'], 'context corpus cap')
                request = parse_json(event['context']['request'])
                validate_request_envelope(request, manifest, corpus['output_cap'], live)
                checked(type(request['messages']) is list and len(request['messages']) == 2
                        and all(type(m) is dict and set(m) == {'role', 'content'} for m in request['messages'])
                        and [m['role'] for m in request['messages']] == ['system', 'user'], 'message envelope')
                user = [m for m in request['messages'] if m['role'] == 'user']
                checked(len(user) == 1, 'user request')
                payload = parse_json(user[0]['content'])
                reports = [parse_json(r['payload']) for r in captured]
                checked(payload['ci_reports'] == reports, 'request evidence payload')
                systems = [m for m in request['messages'] if m['role'] == 'system']
                checked(len(systems) == 1 and systems[0]['content'] == manifest['system_prompt'], 'system prompt')
                binding_row = db.execute('''SELECT a.view_epoch,r.ordinal,b.actor,b.scope FROM attempts a
                    JOIN recovery_attempts r ON r.attempt_id=a.id JOIN branches b ON b.id=a.branch_id
                    WHERE a.id=?''', (aid,)).fetchone()
                checked(binding_row is not None, 'attempt binding')
                if binding_row['ordinal'] == 1:
                    checked(set(payload) == {'ci_reports', 'validation_error'} and payload.get('validation_error') == manifest['feedback_prompt'], 'feedback prompt')
                else:
                    checked(set(payload) == {'ci_reports'}, 'unexpected initial feedback')
                expected_reports = definition['reports'] if binding_row['ordinal'] == 0 else (definition['retry_reports'] if definition['retry_reports'] is not None else definition['reports'])
                checked(reports == expected_reports, 'corpus evidence')
                for captured_row in captured:
                    admitted = db.execute('''SELECT admitted FROM admission_events WHERE actor=? AND scope=?
                        AND evidence_id=? AND epoch<=? ORDER BY epoch DESC LIMIT 1''',
                        (binding_row['actor'], binding_row['scope'], captured_row['id'], binding_row['view_epoch'])).fetchone()
                    checked(admitted is not None and admitted[0] == 1, 'captured admission')
            elif phase == 'claimed':
                row = db.execute('''SELECT d.request,t.input_tokens,t.output_cap,t.granted,t.budget_id,r.request AS frozen_request,r.state
                    FROM dispatches d JOIN token_reservations t ON t.attempt_id=d.attempt_id
                    JOIN recovery_attempts r ON r.attempt_id=d.attempt_id
                    WHERE d.attempt_id=?''', (aid,)).fetchone()
                ctx = prepared[aid]
                checked(row is not None and row['request'] == ctx['request'].encode() and row['input_tokens'] == ctx['input_tokens'] and row['output_cap'] == ctx['output_cap'] and row['granted'] == 1, 'claimed request/quote')
                checked(row['budget_id'] == start['budget_id'] and row['state'] == 'sent' and row['frozen_request'] == row['request'], 'durable reservation binding')
                held += row['input_tokens'] + row['output_cap']
            elif phase == 'received':
                checked(type(event['admission_ns']) is int and 0 <= event['admission_ns'] <= event['operation_elapsed_ns'], 'received timing')
                received_admissions[aid] = event['admission_ns']
                row = db.execute('SELECT output,usage FROM deliveries WHERE attempt_id=?', (aid,)).fetchone()
                checked(row is not None and row['output'].decode() == event['response'], 'response bytes')
                checked(type(event['input_tokens']) is int and type(event['output_tokens']) is int and event['output_tokens'] >= 0, 'measured token types')
                checked(row['usage'] == event['input_tokens'] + event['output_tokens'], 'measured usage')
                checked(event['input_tokens'] == prepared[aid]['input_tokens'] and event['output_tokens'] <= prepared[aid]['output_cap'], 'measured quote')
                checked(event['details']['synthetic_response'] is (not live), 'response mode')
                if live:
                    checked(type(event['details']['hit_cap']) is bool and type(event['details']['ended_eos']) is bool
                            and event['details']['hit_cap'] == (event['output_tokens'] == prepared[aid]['output_cap']), 'generation completion flags')
                if not live:
                    checked(event['input_tokens'] == len(prepared[aid]['request'].encode()) and event['output_tokens'] == len(event['response'].encode()), 'fixture byte counts')
            elif phase == 'settled':
                checked(aid not in seen, 'duplicate settled attempt')
                seen.add(aid)
                ctx, attempt = event['context'], event['attempt']
                checked({k: v for k, v in ctx.items() if k != 'elapsed_ms'} == {k: v for k, v in prepared[aid].items() if k != 'elapsed_ms'}, 'settled prepared context')
                row = db.execute('''SELECT r.ordinal,d.output,d.usage,d.disposition,d.applicability,d.validator,d.reason
                    FROM recovery_attempts r JOIN deliveries d ON d.attempt_id=r.attempt_id WHERE r.attempt_id=?''', (aid,)).fetchone()
                checked(row is not None and row['usage'] == attempt['charged_tokens'], 'settled charge')
                checked(event['stored_delivery'] == {key: row[key] for key in ('disposition', 'applicability', 'validator', 'reason')}, 'stored delivery metadata')
                reference = definition['initial_reference' if row['ordinal'] == 0 else 'retry_reference']
                reports = definition['reports'] if row['ordinal'] == 0 else (definition['retry_reports'] if definition['retry_reports'] is not None else definition['reports'])
                checked(reference == reference_for(reports), 'independent reference')
                correct = validate_output(row['output'], reference)
                checked(attempt['correct'] is correct and ctx['consistent'] is (reference is not None), 'validator correctness')
                disposition = row['disposition']
                if disposition in ('stale_view', 'stale_branch') or (disposition == 'invalid_output' and row['applicability'] != 'current'):
                    disposition = 'stale'
                checked(attempt['disposition'] == disposition, 'settled disposition')
                checked(row['disposition'] != 'accepted' or correct, 'accepted invalid output')
                if row['disposition'] == 'invalid_output':
                    checked(row['validator'] == VALIDATOR and row['reason'] == manifest['feedback_prompt'], 'delivery validator/reason')
                admission, receipt = event['admission_ns'], event['receipt_ns']
                checked(type(admission) is int and type(receipt) is int and previous_receipt_ns <= admission <= receipt
                        and admission == received_admissions[aid] and receipt >= last_operation_ns, 'receipt timing order')
                last_operation_ns = receipt
                checked(ctx['elapsed_ms'] == admission // 1_000_000 - previous_receipt_ns // 1_000_000
                        and attempt['elapsed_ms'] == receipt // 1_000_000 - admission // 1_000_000, 'trace timing')
                previous_receipt_ns = receipt
                observed.append((ctx, attempt))
                spent += row['usage']
                held -= ctx['input_tokens'] + ctx['output_cap']
            elif phase not in ('case_started', 'case_completed'):
                raise ValueError(f'unsupported complete event {phase}; bundle not scored')
            if 'ledger' in event:
                checked(event['ledger'] == dict(budget_id=start['budget_id'], limit=case['budget_tokens'], spent=spent, reserved=held), 'event ledger snapshot')
        checked(observed == cells, 'trace/observed cells')
        checked(all(p == ['prepared', 'claimed', 'received', 'settled'] for p in phases.values()), 'event ordering')
        checked(held == 0, 'final hold')
        known_total += spent
    checked(seen == all_dispatches, 'settled coverage')
    checked(db.execute('SELECT COALESCE(SUM(usage),0) FROM deliveries').fetchone()[0] == known_total, 'global charges')
    affordable = [r for r in report['cases'] if r['policy'] == 'affordable_retry']
    checked(sum(r['charged_tokens'] for r in affordable) == known_total and sum(r['attempts'] for r in affordable) == len(seen), 'replay ledger')
    return dict(status='verified_complete', cases=len(trace['cases']), mode=raw['mode'],
                actual_backend_calls=len(received), new_model_calls=raw['new_model_calls'],
                charged_tokens=known_total, outstanding_tokens=0, summary=report['summary'],
                limitations='Internal consistency audit only; model, tokenizer, timing and source provenance are producer attestations, not authenticated execution evidence.')


def audit_bundle(directory):
    directory = Path(directory).resolve()
    raw = read_json(directory / 'trace.json.raw.json')
    require(type(raw) is dict, 'invalid raw artifact object; not scored')
    db_path = directory / 'trace.json.sqlite'
    require(db_path.is_file(), 'missing SQLite ledger; not scored')
    # SQLite mode=ro can still create WAL/SHM sidecars. Accept only clean
    # rollback-journal bundles so even connection setup cannot mutate artifacts.
    with db_path.open('rb') as source:
        header = source.read(100)
    require(len(header) == 100 and header[:16] == b'SQLite format 3\0', 'invalid SQLite header')
    require(header[18:20] == b'\x01\x01' and not any(Path(str(db_path) + suffix).exists()
            for suffix in ('-wal', '-shm', '-journal')), 'WAL or journal sidecar bundle unsupported; not opened or scored')
    try:
        with closing(sqlite3.connect(db_path.as_uri() + '?mode=ro', uri=True)) as db, db:
            db.row_factory = sqlite3.Row
            db.execute('PRAGMA query_only=ON')
            db.execute('BEGIN')
            unresolved = db.execute('''SELECT COUNT(*) FROM recovery_attempts r LEFT JOIN deliveries d ON d.attempt_id=r.attempt_id
                WHERE r.state IN ('planned','reserved','sent') AND d.attempt_id IS NULL''').fetchone()[0]
            if raw.get('status') != 'complete' or not (directory / 'trace.json').is_file() or unresolved:
                return dict(status='incomplete_not_scored', unresolved_attempts=unresolved,
                            reason='Raw-only, incomplete or unresolved bundle; no cost or policy score inferred.')
            return audit_complete(db, raw, read_json(directory / 'trace.json'))
    except (sqlite3.Error, KeyError, TypeError, IndexError, UnicodeError) as error:
        raise ValueError(f'invalid or unsupported bundle; not scored: {error}') from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    try:
        result = audit_bundle(args.directory)
        print(json.dumps(result, indent=2, allow_nan=False))
        if result['status'] != 'verified_complete':
            parser.exit(2)
    except (OSError, ValueError) as error:
        parser.exit(2, f'audit failed: {error}\n')


if __name__ == '__main__':
    main()
