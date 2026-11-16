"""Contract tests for offline, paired recovery-policy replay; no inference."""
import copy
import json
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

from recovery_policy_eval import evaluate, synthetic_fixture


class RecoveryPolicyTests(unittest.TestCase):
    def setUp(self):
        self.trace = synthetic_fixture()

    def row(self, result, case, policy):
        return next(r for r in result['cases'] if r['case_id'] == case and r['policy'] == policy)

    def test_fixture_is_deterministic_and_clearly_not_model_evidence(self):
        self.assertEqual(self.trace, synthetic_fixture())
        result = evaluate(self.trace)
        self.assertEqual(result['mode'], 'synthetic_policy_fixture')
        self.assertEqual(result['new_model_calls'], 0)
        self.assertIn('not model efficacy', result['limitations'])
        self.assertEqual(set(result['summary']), {'calibration', 'heldout'})

    def test_paired_policies_use_shared_initial_charge_and_policy_specific_retry(self):
        result = evaluate(self.trace)
        stop = self.row(result, 'repair', 'always_stop')
        retry = self.row(result, 'repair', 'affordable_retry')
        gated = self.row(result, 'repair', 'evidence_gated_retry')
        self.assertEqual((stop['charged_tokens'], stop['abstained']), (7, True))
        self.assertEqual((retry['charged_tokens'], retry['success'], retry['attempts']), (15, True, 2))
        self.assertEqual((gated['charged_tokens'], gated['success']), (15, True))

    def test_gate_is_pre_response_consistency_not_correctness(self):
        case = self.trace['cases'][1]
        for cell in case['retries'].values():
            cell['context']['consistent'] = False
        result = evaluate(self.trace)
        self.assertEqual(self.row(result, 'repair', 'evidence_gated_retry')['reason'], 'inconsistent_evidence')
        self.assertTrue(self.row(result, 'repair', 'affordable_retry')['success'])

    def test_correct_late_result_is_charged_but_not_on_time_success(self):
        case = self.trace['cases'][1]
        for cell in case['retries'].values():
            cell['attempt']['elapsed_ms'] = 100
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertTrue(row['correct'])
        self.assertFalse(row['on_time'])
        self.assertFalse(row['success'])
        self.assertFalse(row['abstained'])
        self.assertEqual(row['charged_tokens'], 15)

    def test_uncertain_initial_keeps_full_hold_without_retry_or_fabricated_charge(self):
        case = self.trace['cases'][1]
        case['initial'].update(disposition='uncertain', correct=None, charged_tokens=None)
        for policy in ('always_stop', 'affordable_retry', 'evidence_gated_retry'):
            row = self.row(evaluate(self.trace), 'repair', policy)
            self.assertEqual((row['charged_tokens'], row['reserved_tokens'], row['attempts']), (0, 12, 1))
            self.assertEqual(row['reason'], 'uncertain')
            self.assertFalse(row['abstained'])

    def test_uncertain_retry_preserves_initial_charge_and_retry_hold(self):
        for cell in self.trace['cases'][1]['retries'].values():
            cell['attempt'].update(disposition='uncertain', correct=None, charged_tokens=None)
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertEqual((row['charged_tokens'], row['reserved_tokens']), (7, 13))

    def test_budget_uses_full_fresh_quote_not_actual_future_charge(self):
        case = self.trace['cases'][1]
        case['budget_tokens'] = 19  # 7 spent + 13 quoted is one too many.
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertEqual((row['reason'], row['charged_tokens'], row['reserved_tokens']), ('budget_denied', 7, 0))
        case['budget_tokens'] = 20
        self.assertTrue(self.row(evaluate(self.trace), 'repair', 'affordable_retry')['success'])

    def test_missing_required_paired_cell_is_not_inferred_from_other_policy(self):
        self.trace['cases'][1]['retries']['affordable_retry']['attempt'] = None
        with self.assertRaisesRegex(ValueError, 'missing paired retry'):
            evaluate(self.trace)

    def test_prompt_or_evidence_mismatch_is_rejected_even_for_unused_cells(self):
        for field in ('request', 'evidence_id'):
            trace = copy.deepcopy(self.trace)
            trace['cases'][0]['retries']['affordable_retry']['attempt'][field] = 'wrong'
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'context mismatch'):
                evaluate(trace)

    def test_invalid_schema_accounting_types_and_duplicate_cases_are_rejected(self):
        mutations = [
            lambda t: t.update(mode='live'),
            lambda t: t.update(extra=1),
            lambda t: t['cases'][0].update(budget_tokens=True),
            lambda t: t['cases'][0]['initial'].update(charged_tokens=999),
            lambda t: t['cases'][0]['initial'].update(elapsed_ms=float('nan')),
            lambda t: t['cases'][0]['initial'].update(correct='yes'),
            lambda t: t['cases'][0]['initial'].update(disposition='uncertain'),
            lambda t: t['cases'].append(copy.deepcopy(t['cases'][0])),
            lambda t: t['cases'][0].update(split='test'),
        ]
        for mutation in mutations:
            trace = copy.deepcopy(self.trace)
            mutation(trace)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                evaluate(trace)

    def test_heldout_labels_cannot_change_calibration_or_decisions(self):
        before = evaluate(self.trace)
        for case in self.trace['cases']:
            if case['split'] == 'heldout':
                for cell in case['retries'].values():
                    cell['attempt']['correct'] = False
        after = evaluate(self.trace)
        self.assertEqual(before['summary']['calibration'], after['summary']['calibration'])
        self.assertEqual([(r['attempts'], r['reason']) for r in before['cases']],
                         [(r['attempts'], r['reason']) for r in after['cases']])

    def test_initial_acceptance_stale_and_deadline_never_trigger_retry(self):
        for disposition, elapsed, reason in [('accepted', 5, 'accepted'), ('stale', 5, 'stale'), ('invalid_output', 100, 'deadline')]:
            trace = copy.deepcopy(self.trace)
            trace['cases'][1]['initial'].update(disposition=disposition, elapsed_ms=elapsed)
            row = self.row(evaluate(trace), 'repair', 'affordable_retry')
            self.assertEqual((row['attempts'], row['reason']), (1, reason))

    def test_initial_admission_budget_and_deadline_can_abstain_without_outcome(self):
        for field, value, reason in [('admitted', False, 'ineligible_evidence'),
                                     ('elapsed_ms', 100, 'deadline'),
                                     ('input_tokens', 40, 'budget_denied')]:
            trace = copy.deepcopy(self.trace)
            case = trace['cases'][1]
            case['initial_context'][field] = value
            case['initial'] = None
            row = self.row(evaluate(trace), 'repair', 'affordable_retry')
            self.assertEqual((row['attempts'], row['charged_tokens'], row['reserved_tokens']), (0, 0, 0))
            self.assertEqual(row['reason'], reason)

    def test_retry_preparation_admission_deadline_and_exact_receipt_boundary(self):
        case = self.trace['cases'][1]
        for cell in case['retries'].values():
            cell['context']['elapsed_ms'] = 94  # Initial prep+receipt is 6.
            cell['attempt'] = None
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertEqual((row['reason'], row['charged_tokens']), ('deadline', 7))
        trace = synthetic_fixture()
        for cell in trace['cases'][1]['retries'].values():
            cell['attempt']['elapsed_ms'] = 93  # 1+5+1+93 = 100.
        self.assertTrue(self.row(evaluate(trace), 'repair', 'affordable_retry')['success'])

    def test_ineligible_retry_is_not_dispatched_even_when_affordable(self):
        for cell in self.trace['cases'][1]['retries'].values():
            cell['context']['admitted'] = False
            cell['attempt'] = None
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertEqual((row['reason'], row['charged_tokens']), ('ineligible_evidence', 7))

    def test_shared_context_outcome_and_family_integrity(self):
        for mutation in (
            lambda t: t['cases'][1]['retries']['affordable_retry']['context'].update(consistent=False),
            lambda t: t['cases'][1]['retries']['affordable_retry']['attempt'].update(elapsed_ms=8),
            lambda t: t['cases'][1].update(family=t['cases'][0]['family']),
            lambda t: t['cases'][1].update(split='calibration'),
            lambda t: t['cases'][1]['retries']['affordable_retry']['attempt'].update(input_tokens=6),
        ):
            trace = copy.deepcopy(self.trace)
            mutation(trace)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                evaluate(trace)

    def test_unknown_hold_makes_efficiency_unavailable(self):
        for cell in self.trace['cases'][1]['retries'].values():
            cell['attempt'].update(disposition='uncertain', correct=None, charged_tokens=None)
        summary = evaluate(self.trace)['summary']['heldout']['affordable_retry']
        self.assertIsNone(summary['successes_per_charged_token'])
        self.assertEqual(summary['reserved_tokens'], 13)
        self.assertEqual(summary['on_time'], 0)

    def test_second_invalid_reply_stops_and_charges_both_calls(self):
        for cell in self.trace['cases'][1]['retries'].values():
            cell['attempt'].update(disposition='invalid_output', correct=False)
        row = self.row(evaluate(self.trace), 'repair', 'affordable_retry')
        self.assertEqual((row['attempts'], row['charged_tokens'], row['reason']), (2, 15, 'retry_exhausted'))
        self.assertTrue(row['abstained'])

    def test_cli_rejects_duplicate_keys_and_emits_reproducible_fixture(self):
        script = Path(__file__).with_name('recovery_policy_eval.py')
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder, 'trace.json')
            path.write_text('{"schema_version":1,"schema_version":1}')
            run = subprocess.run([sys.executable, str(script), str(path)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn('duplicate JSON key', run.stderr)
        run = subprocess.run([sys.executable, str(script), '--emit-fixture'], capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(run.stdout), self.trace)

    def test_v2_terminal_initial_allows_absent_retry_proposals(self):
        for disposition in ('accepted', 'stale', 'uncertain'):
            trace = copy.deepcopy(self.trace)
            trace['schema_version'] = 2
            case = trace['cases'][1]
            case['initial'].update(disposition=disposition, correct=False)
            if disposition == 'uncertain':
                case['initial'].update(correct=None, charged_tokens=None)
            case['retries'] = dict.fromkeys(case['retries'])
            result = evaluate(trace)
            self.assertEqual(result['schema_version'], 2)
            row = self.row(result, 'repair', 'affordable_retry')
            self.assertEqual((row['attempts'], row['reason']), (1, disposition))
            self.assertEqual(row['reserved_tokens'], 12 if disposition == 'uncertain' else 0)

    def test_v2_invalid_before_deadline_requires_retry_context_even_with_no_remaining_budget(self):
        for budget in (30, 12):
            trace = copy.deepcopy(self.trace)
            trace['schema_version'] = 2
            case = trace['cases'][1]
            case['budget_tokens'] = budget
            case['retries'] = dict.fromkeys(case['retries'])
            with self.assertRaisesRegex(ValueError, 'retry context required'):
                evaluate(trace)

    def test_v2_one_sided_null_and_v1_null_remain_invalid(self):
        for version, null_keys in ((1, ('affordable_retry', 'evidence_gated_retry')),
                                   (2, ('affordable_retry',)), (2, ('evidence_gated_retry',))):
            trace = copy.deepcopy(self.trace)
            trace['schema_version'] = version
            for key in null_keys:
                trace['cases'][0]['retries'][key] = None
            with self.subTest(version=version, keys=null_keys), self.assertRaises(ValueError):
                evaluate(trace)

    def test_v2_initial_admission_denial_allows_null_retry_cells(self):
        for field, value, reason in [('admitted', False, 'ineligible_evidence'),
                                     ('elapsed_ms', 100, 'deadline'),
                                     ('input_tokens', 40, 'budget_denied')]:
            trace = copy.deepcopy(self.trace)
            trace['schema_version'] = 2
            case = trace['cases'][1]
            case['initial_context'][field] = value
            case['initial'] = None
            case['retries'] = dict.fromkeys(case['retries'])
            row = self.row(evaluate(trace), 'repair', 'affordable_retry')
            self.assertEqual((row['attempts'], row['charged_tokens'], row['reason']), (0, 0, reason))

    def test_v2_null_retry_timing_uses_cumulative_receipt_and_exact_deadline(self):
        for duration in (98, 99, 100):
            trace = copy.deepcopy(self.trace)
            trace['schema_version'] = 2
            case = trace['cases'][1]
            case['initial']['elapsed_ms'] = duration
            case['retries'] = dict.fromkeys(case['retries'])
            if duration == 98:  # 1 preparation + 98 receipt is before deadline.
                with self.assertRaisesRegex(ValueError, 'retry context required'):
                    evaluate(trace)
            else:
                row = self.row(evaluate(trace), 'repair', 'affordable_retry')
                self.assertEqual((row['reason'], row['charged_tokens']), ('deadline', 7))
                self.assertEqual(row['on_time'], duration == 99)

    def test_v2_does_not_hide_missing_initial_outcome_or_weaken_paired_validation(self):
        trace = copy.deepcopy(self.trace)
        trace['schema_version'] = 2
        trace['cases'][1]['initial'] = None
        trace['cases'][1]['retries'] = dict.fromkeys(trace['cases'][1]['retries'])
        with self.assertRaises(ValueError):
            evaluate(trace)
        trace = copy.deepcopy(self.trace)
        trace['schema_version'] = 2
        trace['cases'][1]['retries']['affordable_retry']['context']['consistent'] = False
        with self.assertRaisesRegex(ValueError, 'paired retry contexts differ'):
            evaluate(trace)

    def test_v2_ordinary_replay_matches_v1_scores(self):
        original = evaluate(self.trace)
        self.trace['schema_version'] = 2
        newer = evaluate(self.trace)
        self.assertEqual(original['cases'], newer['cases'])
        self.assertEqual(original['summary'], newer['summary'])


class V2ConversionModeTests(unittest.TestCase):
    def test_explicit_conversion_does_not_change_default_output(self):
        path = Path(__file__).with_name('recovery_policy_eval.py')
        default = subprocess.run([sys.executable, str(path), '--fixture'],
                                 capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(default.stdout), evaluate(synthetic_fixture()))
        converted = subprocess.run([sys.executable, str(path), '--fixture', '--convert-v2'],
                                   capture_output=True, text=True)
        self.assertEqual(converted.returncode, 0, converted.stderr)
        result = json.loads(converted.stdout)
        self.assertEqual(result['format'], 'policy-evaluation-conversion/2')
        self.assertEqual(result['status'], 'Incomplete')
        self.assertEqual(json.loads(default.stdout)['policy_version'], 'frozen-v1')


class ObservedConversionTimingTests(unittest.TestCase):
    def test_explicit_conversion_rejects_changed_preparation_with_same_completion(self):
        from test_policy_evaluation_v2 import legacy_observed_comparison
        old, comparison = legacy_observed_comparison()
        script = Path(__file__).with_name('recovery_policy_eval.py')
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'legacy.json'
            supplied = Path(directory) / 'complete.json'
            source.write_text(json.dumps(old))
            supplied.write_text(json.dumps(comparison))
            command = [sys.executable, str(script), str(source), '--convert-v2', '--comparison-v2', str(supplied)]
            good = subprocess.run(command, capture_output=True, text=True, check=True)
            self.assertEqual(json.loads(good.stdout)['status'], 'Complete')
            for side in ('baseline_rows', 'candidate_rows'):
                for row in comparison[side]:
                    row['attempts'][0]['start_ms'] = 3
            supplied.write_text(json.dumps(comparison))
            bad = subprocess.run(command, capture_output=True, text=True, check=True)
            self.assertEqual(json.loads(bad.stdout)['status'], 'Incomplete')
            self.assertEqual(evaluate(old)['policy_version'], 'frozen-v1')


class ActivationFixtureTests(unittest.TestCase):
    def command(self, mode):
        return subprocess.run([sys.executable, str(Path(__file__).with_name('recovery_policy_eval.py')), mode],
                              capture_output=True, text=True)

    def test_separate_fixture_supplies_complete_synthetic_baseline_and_comparison(self):
        from policy_evaluation_v2 import evaluate_policy_comparison
        completed = self.command('--activation-fixture')
        self.assertEqual(completed.returncode, 0, completed.stderr)
        fixture = json.loads(completed.stdout)
        self.assertEqual(fixture['format'], 'policy-activation-fixture/1')
        self.assertEqual(fixture['new_model_calls'], 0)
        self.assertIn('not feasible named-policy replay', fixture['limitations'])
        paired = fixture['evaluation']
        self.assertEqual(paired['purpose'], 'fixture/recovery-policy')
        self.assertEqual(evaluate_policy_comparison(paired)['decision'], 'Eligible')
        baseline = copy.deepcopy(paired)
        baseline.update(candidate_id=paired['baseline_id'], candidate_sha256=paired['baseline_sha256'],
                        candidate_rows=copy.deepcopy(paired['baseline_rows']))
        self.assertEqual(evaluate_policy_comparison(baseline), {'decision': 'Refused', 'reason': 'NoImprovement'})
        self.assertTrue(all(r['simulated'] and not r['attempts'] for r in paired['baseline_rows'] + paired['candidate_rows']))

    def test_actual_archived_v1_refuses_at_conversion_without_invented_attestation(self):
        import hashlib
        import zipfile
        from policy_evaluation_v2 import convert_legacy
        archive = Path(__file__).resolve().parents[1] / 'docs/evaluations/recovery-policy-v1-2026-11-14.zip'
        completed = self.command('--archived-v1-refusal')
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        with zipfile.ZipFile(archive) as source:
            raw = source.read('trace.json')
            archived_summary = json.loads(source.read('summary.json'))
        self.assertEqual(evaluate(json.loads(raw)), archived_summary)
        self.assertEqual(result['archive_sha256'], hashlib.sha256(archive.read_bytes()).hexdigest())
        self.assertEqual(result['trace_sha256'], hashlib.sha256(raw).hexdigest())
        self.assertEqual(result['conversion'], convert_legacy(json.loads(raw)))
        self.assertEqual(result['conversion']['status'], 'Incomplete')
        self.assertFalse(result['promotion_requested'])
        self.assertEqual(result['new_model_calls'], 0)
        self.assertNotIn('evaluation', result)

    def test_fixture_modes_refuse_mixed_legacy_conversion_flags(self):
        script = str(Path(__file__).with_name('recovery_policy_eval.py'))
        for mode in ('--activation-fixture', '--archived-v1-refusal'):
            completed = subprocess.run([sys.executable, script, mode, '--convert-v2'], capture_output=True, text=True)
            self.assertEqual(completed.returncode, 2)


if __name__ == '__main__':
    unittest.main()
