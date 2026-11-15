"""Cross-language export rejection gates; expected bytes come from frozen Python preparation."""
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import incident_routing as routing
import check_incident_store_bridge as bridge


class ParityTests(unittest.TestCase):
    def setUp(self):
        workload = routing.load_workload()
        prepared = routing.prepare(workload, split='all')
        self.report = {
            'schema_version': 1, 'version': 'incident-store-requests/1',
            'mode': 'synthetic_store_incident_fixture', 'new_model_calls': 0,
            'accounting_unit': 'UTF-8 bytes', 'capture': 'Store',
            'freeze_sha256': routing.FREEZE_SHA256,
            'requests': [dict(case_id=r['case_id'], request=r['request'],
                              request_sha256=r['request_sha256'], manifest_sha256='a' * 64,
                              captured_ids=list(range(1, len(c['evidence']) + 2)))
                         for r, c in zip(prepared['requests'], workload['cases']['cases'])],
        }

    def test_accepts_complete_byte_exact_requests(self):
        self.assertEqual(bridge.check(self.report), 12)

    def test_changed_byte_with_recomputed_digest_is_rejected(self):
        self.report['requests'][0]['request'] += ' '
        self.report['requests'][0]['request_sha256'] = hashlib.sha256(
            self.report['requests'][0]['request'].encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, 'request bytes'):
            bridge.check(self.report)

    def test_omitted_case_is_rejected(self):
        self.report['requests'].pop()
        with self.assertRaisesRegex(ValueError, 'case count'):
            bridge.check(self.report)

    def test_reordered_cases_are_rejected(self):
        self.report['requests'][0], self.report['requests'][1] = (
            self.report['requests'][1], self.report['requests'][0])
        with self.assertRaisesRegex(ValueError, 'case order'):
            bridge.check(self.report)

    def test_reordered_reports_with_recomputed_digest_are_rejected(self):
        record = self.report['requests'][0]
        request = json.loads(record['request'])
        user = json.loads(request['messages'][1]['content'])
        user['evidence'].reverse()
        request['messages'][1]['content'] = routing.canonical(user)
        record['request'] = routing.canonical(request)
        record['request_sha256'] = hashlib.sha256(record['request'].encode()).hexdigest()
        with self.assertRaisesRegex(ValueError, 'request bytes'):
            bridge.check(self.report)

    def test_malformed_metadata_records_and_capture_are_rejected(self):
        for index, mutate in enumerate((
            lambda r: r.update(new_model_calls=True),
            lambda r: r.update(mode='live'),
            lambda r: r.update(capture='declared_flags'),
            lambda r: r.update(accounting_unit='tokens'),
            lambda r: r.update(freeze_sha256='0' * 64),
            lambda r: r['requests'][0].update(manifest_sha256='bad'),
            lambda r: r['requests'][0].update(request_sha256='0' * 64),
            lambda r: r['requests'][0].update(captured_ids=[1, 1, 2]),
            lambda r: r['requests'][0].update(request=None),
            lambda r: r['requests'][0].update(extra='unrecognized'),
        )):
            report = copy.deepcopy(self.report)
            mutate(report)
            with self.subTest(mutation=index), self.assertRaises(ValueError):
                bridge.check(report)

    def test_cli_rejects_corrupt_and_duplicate_key_files(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'requests.json'
            for raw, success in (
                (routing.canonical(self.report), True),
                ('{"requests":[],"requests":[]}', False),
                ('{', False),
            ):
                path.write_text(raw, encoding='utf-8')
                result = subprocess.run(
                    [sys.executable, str(Path(bridge.__file__)), '--requests', str(path)],
                    capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, success, result.stderr)
                if success:
                    self.assertEqual(json.loads(result.stdout)['cases_checked'], 12)


if __name__ == '__main__':
    unittest.main()
