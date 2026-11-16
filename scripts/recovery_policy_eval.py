"""Offline paired policy accounting, NOT a model runner or efficacy experiment.

Schema v1 is illustrated completely by synthetic_fixture(). Schema v2 also permits
both retry cells to be null when the initial path cannot unlock a retry.
Exact request strings,
opaque evidence snapshot IDs, and token quotes must match their recorded outcomes.
The producer must attest snapshot identity, eligibility, consistency and labels;
this program cannot verify a tokenizer, actual evidence bytes, or model provenance.
Each policy has an explicit retry cell. Paired contexts and supplied outcomes
must be identical: both policies replay the same observed trajectory when allowed.
Missing required cells are rejected, never filled from another policy. Preparation duration is
context.elapsed_ms; outcome.elapsed_ms starts at dispatch admission. Unknown sent
usage retains the full quote. No external calls, transport, or database writes occur.
"""
import argparse
import json
from pathlib import Path

POLICIES = ('always_stop', 'affordable_retry', 'evidence_gated_retry')
MODES = ('synthetic_policy_fixture', 'recorded_policy_replay')
CONTEXT_KEYS = {'request', 'evidence_id', 'admitted', 'consistent', 'input_tokens', 'output_cap', 'elapsed_ms'}
ATTEMPT_KEYS = {'request', 'evidence_id', 'input_tokens', 'output_cap', 'disposition', 'correct', 'elapsed_ms', 'charged_tokens'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def fields(value, expected, label):
    require(type(value) is dict and set(value) == expected, f'{label}: unexpected or missing fields')


def integer(value, label, minimum=0):
    require(type(value) is int and minimum <= value <= 2**63 - 1, f'{label}: invalid integer')


def text(value, label):
    require(type(value) is str and bool(value.strip()), f'{label}: nonempty string required')


def validate_context(context):
    fields(context, CONTEXT_KEYS, 'context')
    for key in ('request', 'evidence_id'):
        text(context[key], key)
    for key in ('admitted', 'consistent'):
        require(type(context[key]) is bool, f'{key}: boolean required')
    for key in ('input_tokens', 'elapsed_ms'):
        integer(context[key], key)
    integer(context['output_cap'], 'output_cap', 1)
    integer(context['input_tokens'] + context['output_cap'], 'quote')


def validate_attempt(attempt, context):
    validate_context(context)
    if attempt is None:
        return
    fields(attempt, ATTEMPT_KEYS, 'attempt')
    for key in ('request', 'evidence_id', 'input_tokens', 'output_cap'):
        require(type(attempt[key]) is type(context[key]) and attempt[key] == context[key], f'{key}: context mismatch')
    integer(attempt['elapsed_ms'], 'receipt duration')
    require(attempt['disposition'] in ('accepted', 'invalid_output', 'stale', 'uncertain'), 'unknown disposition')
    if attempt['disposition'] == 'uncertain':
        require(attempt['correct'] is None and attempt['charged_tokens'] is None, 'uncertain outcome must have unknown correctness and charge')
    else:
        require(type(attempt['correct']) is bool, 'correct: boolean required')
        integer(attempt['charged_tokens'], 'charged_tokens')
        require(context['input_tokens'] <= attempt['charged_tokens'] <= context['input_tokens'] + context['output_cap'], 'charge outside reservation')
        require(attempt['disposition'] != 'invalid_output' or not attempt['correct'], 'invalid output cannot be correct')


def validate(document):
    fields(document, {'schema_version', 'mode', 'provenance', 'cases'}, 'document')
    require(type(document['schema_version']) is int and document['schema_version'] in (1, 2), 'unsupported schema version')
    require(document['mode'] in MODES, 'unsupported mode')
    text(document['provenance'], 'provenance')
    require(type(document['cases']) is list and document['cases'], 'nonempty cases required')
    seen = set()
    splits = set()
    families = {}
    for case in document['cases']:
        fields(case, {'case_id', 'family', 'split', 'budget_tokens', 'deadline_ms', 'initial_context', 'initial', 'retries'}, 'case')
        text(case['case_id'], 'case_id')
        require(case['case_id'] not in seen, 'duplicate case_id')
        seen.add(case['case_id'])
        require(case['split'] in ('calibration', 'heldout'), 'unknown split')
        splits.add(case['split'])
        text(case['family'], 'family')
        require(families.setdefault(case['family'], case['split']) == case['split'], 'family crosses splits')
        integer(case['budget_tokens'], 'budget_tokens')
        integer(case['deadline_ms'], 'deadline_ms', 1)
        validate_attempt(case['initial'], case['initial_context'])
        fields(case['retries'], set(POLICIES[1:]), 'retries')
        cells = list(case['retries'].values())
        if any(cell is None for cell in cells):
            require(document['schema_version'] == 2, 'null retry cells require schema v2')
            require(all(cell is None for cell in cells), 'paired retry cells must both be null')
            context, initial = case['initial_context'], case['initial']
            initial_admitted = (context['admitted']
                                and context['elapsed_ms'] < case['deadline_ms']
                                and context['input_tokens'] + context['output_cap'] <= case['budget_tokens'])
            if initial_admitted:
                require(initial is not None, 'missing initial outcome')
                retry_required = (initial['disposition'] == 'invalid_output'
                                  and context['elapsed_ms'] + initial['elapsed_ms'] < case['deadline_ms'])
                require(not retry_required, 'retry context required after initial invalid output')
            continue
        for cell in case['retries'].values():
            fields(cell, {'context', 'attempt'}, 'retry cell')
            validate_attempt(cell['attempt'], cell['context'])
        left, right = (case['retries'][p] for p in POLICIES[1:])
        require(left['context'] == right['context'], 'paired retry contexts differ')
        if left['attempt'] is not None and right['attempt'] is not None:
            require(left['attempt'] == right['attempt'], 'paired retry outcomes differ')
    require(splits == {'calibration', 'heldout'}, 'both calibration and heldout splits required')


def replay(case, policy):
    row = dict(case_id=case['case_id'], split=case['split'], policy=policy,
               attempts=0, charged_tokens=0, reserved_tokens=0, elapsed_ms=0,
               correct=False, on_time=False, success=False, abstained=False, reason='')

    def consume(context, attempt, retry):
        # Gate before response access. Quotes and preparation timing are context,
        # never obtained from prospective actual usage/correctness.
        row['elapsed_ms'] += context['elapsed_ms']
        if not context['admitted']:
            row.update(reason='ineligible_evidence', abstained=True)
            return False
        if retry and policy == 'evidence_gated_retry' and not context['consistent']:
            row.update(reason='inconsistent_evidence', abstained=True)
            return False
        if row['elapsed_ms'] >= case['deadline_ms']:
            row.update(reason='deadline', abstained=True)
            return False
        quote = context['input_tokens'] + context['output_cap']
        if quote > case['budget_tokens'] - row['charged_tokens'] - row['reserved_tokens']:
            row.update(reason='budget_denied', abstained=True)
            return False
        require(attempt is not None, 'missing paired retry outcome' if retry else 'missing initial outcome')
        row.update(correct=False, on_time=False)
        row['attempts'] += 1
        row['elapsed_ms'] += attempt['elapsed_ms']
        row['reason'] = attempt['disposition']
        if attempt['disposition'] == 'uncertain':
            row['reserved_tokens'] += quote
            return False
        row['charged_tokens'] += attempt['charged_tokens']
        row['correct'] = attempt['correct']
        row['on_time'] = row['elapsed_ms'] <= case['deadline_ms']
        row['success'] = attempt['disposition'] == 'accepted' and row['correct'] and row['on_time']
        return attempt['disposition'] == 'invalid_output'

    invalid = consume(case['initial_context'], case['initial'], False)
    if invalid:
        if policy == 'always_stop':
            row.update(reason='policy_stop', abstained=True)
        elif row['elapsed_ms'] >= case['deadline_ms']:
            row.update(reason='deadline', abstained=True)
        else:
            cell = case['retries'][policy]
            invalid_retry = consume(cell['context'], cell['attempt'], True)
            if invalid_retry:
                row.update(reason='retry_exhausted', abstained=True)
    return row


def evaluate(document):
    """Validate a complete paired corpus, then score frozen policies separately."""
    validate(document)
    rows = [replay(case, policy) for case in document['cases'] for policy in POLICIES]
    summary = {}
    for split in ('calibration', 'heldout'):
        summary[split] = {}
        for policy in POLICIES:
            selected = [r for r in rows if r['split'] == split and r['policy'] == policy]
            totals = {key: sum(r[key] for r in selected) for key in (
                'attempts', 'charged_tokens', 'reserved_tokens', 'correct', 'on_time', 'success', 'abstained')}
            totals.update(cases=len(selected), success_rate=totals['success'] / len(selected),
                          correct_late=sum(r['correct'] and not r['on_time'] for r in selected),
                          successes_per_charged_token=(totals['success'] / totals['charged_tokens']
                              if totals['charged_tokens'] and not totals['reserved_tokens'] else None))
            summary[split][policy] = totals
    return dict(schema_version=document['schema_version'], mode=document['mode'], provenance=document['provenance'],
                new_model_calls=0, policy_version='frozen-v1', cases=rows, summary=summary,
                limitations='Synthetic fixtures demonstrate mechanics, not model efficacy. Recorded replay trusts producer attestations and only scores supplied exact-context outcomes; no counterfactual inference. Policy totals are separate hypothetical paths, not additive research expenditure. No calibration fitting or heldout tuning.')


def synthetic_fixture():
    """Small deterministic mechanics-only corpus, with paired supplied outcomes."""
    def context(request, evidence='snapshot-v1', count=4, cap=8):
        return dict(request=request, evidence_id=evidence, admitted=True, consistent=True,
                    input_tokens=count, output_cap=cap, elapsed_ms=1)

    def attempt(ctx, disposition, correct, charge):
        return dict(request=ctx['request'], evidence_id=ctx['evidence_id'],
                    input_tokens=ctx['input_tokens'], output_cap=ctx['output_cap'],
                    disposition=disposition, correct=correct, elapsed_ms=5, charged_tokens=charge)

    cases = []
    for name, split, disposition in [('first-pass', 'calibration', 'accepted'), ('repair', 'heldout', 'invalid_output')]:
        initial_context = context(f'{name}: judge snapshot-v1')
        retries = {}
        for policy, count, charge in [('affordable_retry', 5, 8), ('evidence_gated_retry', 5, 8)]:
            ctx = context(f'{name}: judge snapshot-v2 with validation feedback', 'snapshot-v2', count)
            retries[policy] = dict(context=ctx, attempt=attempt(ctx, 'accepted', True, charge))
        cases.append(dict(case_id=name, family=name, split=split, budget_tokens=30, deadline_ms=100,
                          initial_context=initial_context,
                          initial=attempt(initial_context, disposition, disposition == 'accepted', 7), retries=retries))
    return dict(schema_version=1, mode='synthetic_policy_fixture',
                provenance='Hand-authored byte-independent token counts and durations; no model measurements.', cases=cases)


def activation_fixture():
    """Complete hand-authored registry counters, never feasible policy trajectories."""
    from policy_evaluation_v2 import canonical, sha, SELECTION_CONTRACT_SHA256

    def record(raw):
        return dict(bytes=list(raw), sha256=sha(raw))

    compatibility = sha(b'policy-report-fixture/1:one-policy-then-reports;fixture-byte-runtime/1')
    policies = [dict(policy_name=name, artifact=record(canonical(dict(
        schema_version=1, policy_name=name, purpose='fixture/recovery-policy',
        compatibility_contract_sha256=compatibility)))) for name in POLICIES]
    scorer = sha(b'synthetic-opaque-scorer/1')
    label = record(canonical(dict(answerable=True, admissible_support=True,
        normalized_format='opaque-bytes/1', scorer_sha256=scorer, expected_normalized=list(b'yes'))))
    opportunities = [dict(policy_name=p['policy_name'], opportunities=[dict(
        opportunity_id=p['policy_name'] + '/' + str(n), request=record(('synthetic request ' + str(n)).encode()),
        evidence_manifest=record(b'synthetic evidence'), admitted=True, consistent=True,
        input_bound=4, output_cap=6, start_min_ms=0, start_max_ms=99)
        for n in range(1 if p['policy_name'] == 'always_stop' else 2)]) for p in policies]
    case = dict(case_id='synthetic-counter-case', family_id='synthetic-family', split_id='heldout',
        initial_evidence=record(b'synthetic evidence'), label=label, budget=30, deadline_ms=100,
        timing_start_ms=0, timing_end_ms=200, policy_opportunities=opportunities)
    manifest = dict(format='policy-comparison/1', actor='policy-activation-fixture', scope='ci-report',
        purpose='fixture/recovery-policy', provenance='synthetic_registry_mechanics',
        accounting_unit='fixture_bytes', compatibility_contract_sha256=compatibility,
        policies=policies, cases=[case], **{key + '_sha256': sha(('synthetic-' + key + '/1').encode())
            for key in ('workload', 'prompt', 'runtime', 'model', 'tokenizer', 'validator')}, scorer_sha256=scorer)
    baseline = dict(case_id=case['case_id'], outcome='Refused', correct=False, eligible=True,
        answerable=True, completed_ms=100, charged=4, reserved_bound=0, false_refusal=True,
        unsafe_acceptance=False, deadline_miss=False, simulated=True, attempts=[])
    candidate = dict(baseline, outcome='Accepted', correct=True, false_refusal=False)
    raw = canonical(manifest)
    evaluation = dict(evaluation_id='synthetic-paired/1', actor=manifest['actor'], scope=manifest['scope'],
        purpose=manifest['purpose'], baseline_id='always-stop/1', candidate_id='affordable-retry/1',
        baseline_sha256=policies[0]['artifact']['sha256'], candidate_sha256=policies[1]['artifact']['sha256'],
        experiment_manifest=list(raw), experiment_sha256=sha(raw), evaluator_version='policy-selection/1',
        evaluator_sha256=SELECTION_CONTRACT_SHA256, provenance=manifest['provenance'],
        baseline_rows=[baseline], candidate_rows=[candidate])
    return dict(format='policy-activation-fixture/1', new_model_calls=0, evaluation=evaluation,
        limitations='Synthetic registry attestations only; not feasible named-policy replay or measured superiority.')


def archived_v1_refusal():
    """Exercise the unchanged converter on the retained actual archive."""
    import zipfile
    from policy_evaluation_v2 import convert_legacy, sha
    archive = Path(__file__).resolve().parents[1] / 'docs/evaluations/recovery-policy-v1-2026-11-14.zip'
    with zipfile.ZipFile(archive) as source:
        raw = source.read('trace.json')
    conversion = convert_legacy(json.loads(raw, object_pairs_hook=unique_object))
    return dict(format='archived-policy-promotion-refusal/1', archive=str(archive),
        archive_sha256=sha(archive.read_bytes()), trace_sha256=sha(raw), conversion=conversion,
        promotion_requested=False, new_model_calls=0,
        limitations='Incomplete conversion supplies no insertable PolicyEvaluation; promotion is withheld before Store insertion.')


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f'duplicate JSON key: {key}')
        result[key] = value
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', nargs='?', type=Path)
    parser.add_argument('--fixture', action='store_true', help='evaluate deterministic synthetic mechanics only')
    parser.add_argument('--emit-fixture', action='store_true', help='print the full synthetic input schema example')
    parser.add_argument('--activation-fixture', action='store_true', help='emit a separate complete synthetic registry fixture')
    parser.add_argument('--archived-v1-refusal', action='store_true', help='show actual archived v1 conversion refusal without inventing facts')
    parser.add_argument('--convert-v2', action='store_true', help='explicit complete-attestation conversion; missing facts remain incomplete')
    parser.add_argument('--comparison-v2', type=Path, help='matching complete v2 comparison attestation for --convert-v2')
    args = parser.parse_args()
    if (args.activation_fixture or args.archived_v1_refusal) and (args.convert_v2 or args.comparison_v2):
        parser.error('registry fixture/refusal modes cannot convert legacy inputs')
    if (args.comparison_v2 and not args.convert_v2) or (args.convert_v2 and args.emit_fixture):
        parser.error('--comparison-v2 requires --convert-v2; conversion cannot emit a fixture')
    if sum((args.trace is not None, args.fixture, args.emit_fixture, args.activation_fixture, args.archived_v1_refusal)) != 1:
        parser.error('choose exactly one input/fixture/refusal mode')
    try:
        if args.activation_fixture or args.archived_v1_refusal:
            result = activation_fixture() if args.activation_fixture else archived_v1_refusal()
            print(json.dumps(result, indent=2, allow_nan=False))
            return
        document = synthetic_fixture() if args.fixture or args.emit_fixture else json.loads(
            args.trace.read_text(), object_pairs_hook=unique_object,
            parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f'nonfinite JSON value: {value}')))
        if args.convert_v2:
            from policy_evaluation_v2 import convert_legacy, parse_policy_evaluation, MAX_BYTES
            comparison = None
            if args.comparison_v2:
                with args.comparison_v2.open('rb') as stream:
                    comparison = parse_policy_evaluation(stream.read(MAX_BYTES + 1))
            result = convert_legacy(document, comparison)
        else:
            result = document if args.emit_fixture else evaluate(document)
        print(json.dumps(result, indent=2, allow_nan=False))
    except (ValueError, OSError, TypeError) as error:
        parser.exit(2, f'error: {error}\n')


if __name__ == '__main__':
    main()
