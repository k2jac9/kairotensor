"""Bounded codec and real precompiled subprocess checks; no build/install helpers."""
import copy
import hashlib
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = (ROOT / 'docs/decision-bundle-v1.md').read_text().split('```json\n')[-1].split('\n```')[0].encode()


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode()


def seal(value):
    value = copy.deepcopy(value)
    value['integrity'].pop('sha256', None)
    value['integrity']['sha256'] = hashlib.sha256(canonical(value)).hexdigest()
    return canonical(value)


class CodecTests(unittest.TestCase):
    def reader(self):
        self.assertTrue((ROOT / 'scripts/decision_bundle_v1.py').is_file(), 'versioned reader must exist')
        return importlib.import_module('decision_bundle_v1')

    def test_independent_golden_and_transport_identity(self):
        reader = self.reader()
        self.assertEqual(len(GOLDEN), 825)
        value = reader.parse_bundle(GOLDEN)
        result = reader.inspect_bundle(value)
        self.assertEqual(result['source_bundle_sha256'], '569b219c44295bb8e1a42358b1bbbd66c153ff379694dd55ef5decf229dbe471')
        self.assertEqual(reader.parse_bundle(json.dumps(value).encode()), value)
        self.assertEqual(result['rust_ledger_validation'], 'not_performed')
        self.assertEqual(result['validator_execution'], 'not_performed')
        self.assertEqual(result['historical_timing'], 'not_reproduced')
        self.assertEqual(result['current_access'], 'not_authenticated')

    def test_binary_bytes_and_tamper(self):
        reader = self.reader()
        raw = b'\x00\xff\x80\r\n'
        envelope = dict(hex=raw.hex(), byte_len=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        self.assertEqual(reader.decode_envelope(envelope), raw)
        for change in ({'hex': raw.hex().upper()}, {'byte_len': True}, {'sha256': '0' * 64},
                       {'byte_len': 4194305}, {'hex': '00 '}, {'extra': 1}, {'version': 'bypass'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                reader.decode_envelope(envelope | change)

    def test_duplicate_depth_size_and_digest_refuse(self):
        reader = self.reader()
        for raw in (b'{"a":1,"a":2}', b'[' * 65 + b'0' + b']' * 65,
                    b' ' * (16 * 1024 * 1024 + 1), GOLDEN.replace(b'"actor":"\xc3\xa9"', b'"actor":"x"', 1)):
            with self.subTest(size=len(raw)), self.assertRaises(ValueError):
                reader.parse_bundle(raw)

    def test_redacted_and_unsupported_version_refuse(self):
        reader = self.reader()
        with self.assertRaisesRegex(ValueError, 'redacted'):
            reader.parse_bundle(b'{"status":"redacted","bundle":{"requested_attempt":1,"reason":"unavailable"}}')
        value = json.loads(GOLDEN)
        value['schema_version'] = 2
        with self.assertRaises(ValueError):
            reader.parse_bundle(seal(value))

    def test_bounded_read_before_materialization(self):
        reader = self.reader()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'oversize'
            with path.open('wb') as output:
                output.truncate(16 * 1024 * 1024 + 1)
            with self.assertRaises(ValueError):
                reader.read_bundle(path)


class ProcessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.runner = os.environ.get('KAIROTENSOR_DECISION_REPLAY', '')
        if not cls.runner or not Path(cls.runner).is_file() or not os.access(cls.runner, os.X_OK):
            raise AssertionError('KAIROTENSOR_DECISION_REPLAY must bind a verified precompiled executable; subprocess checks are required')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'bundle.json'

    def run_cli(self, *args):
        return subprocess.run([self.runner, *map(str, args)], capture_output=True, timeout=30)

    def test_actual_incident_export_then_separate_replay_and_python(self):
        exported = self.run_cli('export-fixture', self.path)
        self.assertEqual(exported.returncode, 0, exported.stderr)
        before = hashlib.sha256(self.path.read_bytes()).hexdigest()
        value = json.loads(self.path.read_bytes())
        self.assertEqual(value['export_purpose'], 'current_read')
        self.assertIsNotNone(value['attempts'][0]['immutable_manifest'])
        self.assertEqual(value['attempts'][0]['inputs'][0]['role'], 'context')
        report = self.run_cli('replay', self.path)
        self.assertEqual(report.returncode, 0, report.stderr)
        report = json.loads(report.stdout)
        self.assertEqual(json.loads(exported.stdout), report)
        self.assertEqual(report['consistency'], 'matched')
        self.assertEqual(report['reconstruction'], 'matched')
        self.assertEqual(report['validation'], 'matched')
        self.assertEqual(report['recorded_disposition'], 'accepted')
        self.assertIn('historical_timing_unavailable', report['missing_requirements'])
        python = subprocess.run([sys.executable, str(ROOT / 'scripts/audit_recovery_bundle.py'), '--decision-v1', str(self.path)], capture_output=True, timeout=30)
        self.assertEqual(python.returncode, 0, python.stderr)
        facts = json.loads(python.stdout)
        self.assertEqual(facts['source_bundle_sha256'], report['source_bundle_sha256'])
        self.assertEqual(facts['validator_execution'], 'not_performed')
        self.assertEqual(before, hashlib.sha256(self.path.read_bytes()).hexdigest())

    def test_binary_preservation_and_unknown_validator(self):
        value = json.loads(GOLDEN)
        raw = b'\x00\xff\x80\r\n'
        envelope = dict(hex=raw.hex(), byte_len=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        value['evidence'] = [dict(occurrence_id=1, source=envelope, payload=envelope,
                                  authorized_scopes=[dict(actor='é', scope='s')])]
        value['attempts'][0]['candidates'] = [dict(position=0, evidence_id=1)]
        value['attempts'][0]['inputs'] = [dict(position=0, evidence_id=1, role='unknown')]
        value['attempts'][0]['validator'] = dict(version='unavailable/99', provenance='unknown')
        value['attempts'][0]['delivery'] = dict(state='recorded', raw=envelope,
                                                disposition='accepted', usage=0, rejection=None)
        self.path.write_bytes(seal(value))
        before = self.path.read_bytes()
        result = self.run_cli('replay', self.path)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report['validation'], 'not_reproducible')
        self.assertIn('validator_unavailable', report['missing_requirements'])
        reader = importlib.import_module('decision_bundle_v1')
        decoded = reader.read_bundle(self.path)
        self.assertEqual(reader.decode_envelope(decoded['evidence'][0]['payload']), raw)
        self.assertEqual(reader.inspect_bundle(decoded)['declared_validators'], ['unavailable/99'])
        self.assertEqual(before, self.path.read_bytes())

    def test_exclusive_export_unknown_mode_and_redacted_refusal(self):
        self.path.write_bytes(b'preserved')
        self.assertNotEqual(self.run_cli('export-fixture', self.path).returncode, 0)
        self.assertEqual(self.path.read_bytes(), b'preserved')
        absent = self.path.with_name('absent')
        for mode in ('live', 'generate', 'unknown'):
            self.assertNotEqual(self.run_cli(mode, absent).returncode, 0)
            self.assertFalse(absent.exists())
        self.path.write_bytes(b'{"status":"redacted","bundle":{"requested_attempt":1,"reason":"unavailable"}}')
        before = self.path.read_bytes()
        self.assertNotEqual(self.run_cli('replay', self.path).returncode, 0)
        self.assertEqual(self.path.read_bytes(), before)

    def test_python_mode_is_explicit_and_refuses_redaction(self):
        script = str(ROOT / 'scripts/audit_recovery_bundle.py')
        self.path.write_bytes(GOLDEN)
        for args in ([str(self.path)], ['--decision-v1', str(self.path), str(self.path.parent)]):
            result = subprocess.run([sys.executable, script, *args], capture_output=True, timeout=30)
            self.assertNotEqual(result.returncode, 0)
        self.path.write_bytes(b'{"status":"redacted","bundle":{"requested_attempt":1,"reason":"unavailable"}}')
        result = subprocess.run([sys.executable, script, '--decision-v1', str(self.path)], capture_output=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'redacted', result.stderr)
        self.assertEqual(sorted(p.name for p in self.path.parent.iterdir()), ['bundle.json'])

    def test_rust_size_depth_and_corruption_refuse_without_mutation(self):
        for raw in (b' ' * (16 * 1024 * 1024 + 1), b'[' * 65 + b'0' + b']' * 65,
                    GOLDEN.replace(b'"scope":"s"', b'"scope":"x"', 1)):
            self.path.write_bytes(raw)
            self.assertNotEqual(self.run_cli('replay', self.path).returncode, 0)
            self.assertEqual(self.path.read_bytes(), raw)


if __name__ == '__main__':
    unittest.main()
