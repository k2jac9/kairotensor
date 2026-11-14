"""Pure admission contracts; no model, clocks, files or transport involved."""
from dataclasses import replace
import json
import unittest

from recovery_admission import RuntimeProfile, LatencyEstimate, build_latency_estimate, decide_admission


def profile(**changes):
    return replace(RuntimeProfile(model='model', revision='revision', tokenizer_revision='tokenizer',
        runtime_version='torch=x;transformers=y', dtype='bfloat16', decoding='free',
        cpu_profile='a' * 64, threads=4, prompt_sha256='b' * 64, output_cap=96), **changes)


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.profile = profile()
        self.estimate = build_latency_estimate(self.profile, [100, 150], max_input_tokens=200)
        self.args = dict(profile=self.profile, estimate=self.estimate, operation_start_ms=1000,
            now_ms=1200, deadline_ms=1500, safety_margin_ms=25, input_tokens=180, output_cap=96,
            budget_limit=1000, spent_tokens=300, reserved_tokens=50, initial_disposition='invalid_output')

    def test_maximum_plus_explicit_margin_and_full_quote(self):
        decision = decide_admission(**self.args)
        self.assertTrue(decision.admitted)
        self.assertEqual(decision.reason, 'admitted')
        self.assertEqual(decision.estimated_finish_ms, 1375)
        self.assertEqual(decision.available_tokens, 650)
        self.assertEqual(self.estimate.max_duration_ms, 150)
        self.assertEqual(self.estimate.sample_count, 2)
        self.assertEqual(self.estimate.estimator, 'calibration_max')
        record = decision.to_dict()
        self.assertEqual(record['profile']['cpu_profile'], 'a' * 64)
        self.assertEqual(record['operation_start_ms'], 1000)
        self.assertEqual(record['deadline_ms'], 1500)
        json.dumps(record, allow_nan=False)

    def test_exact_completion_deadline_is_rejected(self):
        decision = decide_admission(**(self.args | dict(deadline_ms=1375)))
        self.assertFalse(decision.admitted)
        self.assertEqual(decision.reason, 'insufficient_time')
        self.assertEqual(decision.estimated_finish_ms, 1375)
        self.assertTrue(decide_admission(**(self.args | dict(deadline_ms=1376))).admitted)

    def test_expired_initial_and_restarted_clock_cannot_reset_window(self):
        for now in (1500, 1600):
            decision = decide_admission(**(self.args | dict(now_ms=now)))
            self.assertFalse(decision.admitted)
            self.assertEqual(decision.reason, 'deadline_expired')
        for changes in (dict(now_ms=999), dict(deadline_ms=999), dict(deadline_ms=1000)):
            self.assertEqual(decide_admission(**(self.args | changes)).reason, 'invalid_window')
        self.assertTrue(decide_admission(**self.args).admitted)
        self.assertFalse(decide_admission(**(self.args | dict(now_ms=1400))).admitted)

    def test_budget_requires_fresh_input_plus_cap_and_never_releases_holds(self):
        before = dict(self.args)
        self.assertTrue(decide_admission(**(self.args | dict(budget_limit=626))).admitted)
        decision = decide_admission(**(self.args | dict(budget_limit=625)))
        self.assertFalse(decision.admitted)
        self.assertEqual(decision.reason, 'insufficient_budget')
        self.assertEqual(decision.available_tokens, 275)
        self.assertEqual(self.args, before)
        self.assertEqual(decide_admission(**(self.args | dict(reserved_tokens=None))).reason, 'invalid_budget')

    def test_unknown_estimates_and_profiles_have_no_fallback(self):
        self.assertEqual(decide_admission(**(self.args | dict(estimate=None))).reason, 'unknown_estimate')
        self.assertEqual(decide_admission(**(self.args | dict(profile=None))).reason, 'unknown_profile')
        for changes in (dict(dtype='float32'), dict(runtime_version='different'), dict(cpu_profile='c'*64),
                        dict(threads=8), dict(model='other'), dict(revision='other'),
                        dict(tokenizer_revision='other'), dict(decoding='candidate_trie/1'),
                        dict(prompt_sha256='c'*64), dict(output_cap=64)):
            self.assertEqual(decide_admission(**(self.args | dict(profile=profile(**changes)))).reason, 'profile_mismatch')

    def test_estimate_cannot_cover_larger_unmeasured_input_or_different_cap(self):
        for changes in (dict(input_tokens=201), dict(output_cap=95)):
            self.assertEqual(decide_admission(**(self.args | changes)).reason, 'quote_outside_calibration')

    def test_no_retry_after_accepted_stale_or_uncertain_initial(self):
        for state in ('accepted', 'stale', 'uncertain', 'cancelled', None, True):
            self.assertEqual(decide_admission(**(self.args | dict(initial_disposition=state))).reason, 'initial_not_retryable')

    def test_bad_time_and_margin_values_fail_closed(self):
        for key in ('operation_start_ms', 'now_ms', 'deadline_ms', 'safety_margin_ms'):
            for value in (float('nan'), float('inf'), -1, True, None, '100'):
                with self.subTest(key=key, value=value):
                    decision = decide_admission(**(self.args | {key: value}))
                    self.assertFalse(decision.admitted)
                    json.dumps(decision.to_dict(), allow_nan=False)

    def test_bad_quotes_and_accounting_fail_closed(self):
        for key in ('input_tokens', 'output_cap', 'budget_limit', 'spent_tokens', 'reserved_tokens'):
            for value in (-1, True, None, 1.5, float('nan'), float('inf')):
                with self.subTest(key=key, value=value):
                    self.assertFalse(decide_admission(**(self.args | {key: value})).admitted)
        self.assertEqual(decide_admission(**(self.args | dict(spent_tokens=1001))).reason, 'invalid_budget')
        self.assertFalse(decide_admission(**(self.args | dict(output_cap=0))).admitted)

    def test_invalid_calibration_samples_rejected_not_dropped(self):
        for durations in ([], [0], [-1], [True], [float('nan')], [100, float('inf')], ['100'], None):
            with self.subTest(durations=durations), self.assertRaises(ValueError):
                build_latency_estimate(self.profile, durations, max_input_tokens=200)
        for maximum in (-1, True, None, 1.2):
            with self.assertRaises(ValueError):
                build_latency_estimate(self.profile, [100], max_input_tokens=maximum)

    def test_forged_invalid_estimates_are_rejected(self):
        for estimate in (replace(self.estimate, max_duration_ms=0), replace(self.estimate, sample_count=0),
                         replace(self.estimate, max_duration_ms=float('nan')),
                         replace(self.estimate, estimator='p99'), replace(self.estimate, max_input_tokens=-1)):
            self.assertEqual(decide_admission(**(self.args | dict(estimate=estimate))).reason, 'invalid_estimate')

    def test_profile_identity_must_be_known_strictly_typed(self):
        for changes in (dict(model='unknown'), dict(runtime_version=''), dict(dtype=True),
                        dict(threads=True), dict(threads=0), dict(cpu_profile='unknown'),
                        dict(prompt_sha256='not-a-sha'), dict(output_cap=0)):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                profile(**changes)

    def test_fractional_time_components_round_up_conservatively(self):
        estimate = build_latency_estimate(self.profile, [150.1], max_input_tokens=200)
        result = decide_admission(**(self.args | dict(estimate=estimate, now_ms=1200.1,
                                                      safety_margin_ms=25.1, deadline_ms=1378)))
        self.assertEqual(result.estimated_finish_ms, 1378)
        self.assertFalse(result.admitted)

    def test_profile_roundtrip_and_fingerprint_bind_every_field(self):
        self.assertEqual(RuntimeProfile.from_dict(self.profile.to_dict()), self.profile)
        self.assertEqual(len(self.profile.fingerprint), 64)
        self.assertNotEqual(profile(dtype='float32').fingerprint, self.profile.fingerprint)
        for value in ({}, self.profile.to_dict() | {'extra': 1}):
            with self.assertRaises(ValueError):
                RuntimeProfile.from_dict(value)


if __name__ == '__main__':
    unittest.main()
