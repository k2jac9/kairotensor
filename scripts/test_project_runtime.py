"""Launcher boundary tests; fake compiler/wrapper avoid compiling in unit tests."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

LAUNCHER = Path(__file__).with_name('project_runtime.py').resolve()


def identity(path):
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


class ProjectRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True)
        (self.repo / 'Cargo.lock').write_text('fixture lock\n')
        subprocess.run(['git', '-C', str(self.repo), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(self.repo), '-c', 'user.name=Fixture',
                        '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture'], check=True)
        self.commit = subprocess.check_output(['git', '-C', str(self.repo), 'rev-parse', 'HEAD'], text=True).strip()
        self.state = self.root / 'state'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CARGO_', 'RUST', 'PYTHON', 'KAIROTENSOR_'))
                    and k not in {'CC', 'CXX', 'AR', 'LD', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'VIRTUAL_ENV', 'CONDA_PREFIX'}}
        self.env['PYTHONDONTWRITEBYTECODE'] = '1'
        # Isolated Cargo home avoids workstation config influencing the fixture.
        self.env['CARGO_HOME'] = str(self.root / 'cargo-home')
        self.tools = {}
        for key in ['rustc', 'cargo']:
            path = self.root / key
            path.write_text('#!/bin/sh\nprintf "%s\\n" "' + key + ' fixture"\n')
            path.chmod(0o700)
            self.tools[key] = dict(identity(path), version=key + ' fixture', version_args=['--version'])
        python = Path(sys.executable).resolve()
        self.tools['python'] = dict(identity(python), version=subprocess.check_output([str(python), '--version'], text=True).strip(), version_args=['--version'])
        self.wrapper = self.root / 'wrapper'
        self.wrapper.write_text('#!' + str(python) + '\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\nsys.exit(17 if "deny" in sys.argv else 0)\n')
        self.wrapper.chmod(0o700)
        self.review = self.write('review.md', 'independent fixture review\n')
        self.acceptance = self.write('acceptance.json', {'result': 'G1 accepted', 'accepted_executable_source': self.commit,
                                                       'independent_review': identity(self.review)})
        self.manifest = {'source': {'commit': self.commit}, 'lanes': [{'role': 'stable', **self.tools, 'provenance': [identity(self.review)]}],
                         'native_files': [identity(self.review)], 'wrapper': identity(self.wrapper),
                         'storage_guard': identity(self.review), 'cargo_recipe': identity(self.review)}
        self.manifest_path = self.write('manifest.json', self.manifest)
        self.receipt = {'status': 'Pass', 'source': self.manifest['source'], 'role': 'stable',
                        'manifest_sha256': hashlib.sha256(json.dumps(self.manifest, sort_keys=True).encode()).hexdigest(),
                        'tools': {k: {x: v for x, v in t.items() if x != 'version_args'} for k, t in self.tools.items()}}
        for t in self.receipt['tools'].values():
            t['resolved_path'] = str(Path(t['path']).resolve())
        self.receipt_path = self.write('receipt.json', self.receipt)
        self.selection = {'schema': 'kairotensor-project-runtime/1', 'role': 'stable', 'tools': self.tools,
                          'qualification_manifest': identity(self.manifest_path), 'qualification_receipt': identity(self.receipt_path),
                          'independent_review': identity(self.review), 'acceptance': identity(self.acceptance)}
        self.selection_path = self.root / 'selection.json'

    def write(self, name, value):
        path = self.root / name
        path.write_text(value if isinstance(value, str) else json.dumps(value))
        return path

    def run_launcher(self, *args, env=None):
        self.selection_path.write_text(json.dumps(self.selection))
        return subprocess.run([sys.executable, str(LAUNCHER), '--selection', str(self.selection_path),
                               '--state-dir', str(self.state), *args], cwd=self.repo,
                              env=self.env if env is None else env, capture_output=True, text=True)

    def latest(self):
        return json.loads(sorted(self.state.glob('invocations/*/receipt.json'))[-1].read_text())

    def test_selected_parent_and_child_python(self):
        r = self.run_launcher('python', '-c', 'import subprocess; print(subprocess.check_output(["python3", "--version"], text=True).strip())')
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), self.tools['python']['version'])
        probe = self.latest()['python_probe']
        self.assertEqual(probe['parent'], probe['child'])
        self.assertEqual(probe['child']['realpath'], self.tools['python']['path'])

    def test_missing_selection_refuses_without_install(self):
        r = subprocess.run([sys.executable, str(LAUNCHER), '--selection', str(self.root / 'absent'), 'identity'], capture_output=True, text=True, env=self.env)
        self.assertEqual(r.returncode, 2)
        self.assertIn('selection', r.stderr.lower())

    def test_wrong_hash_refuses(self):
        self.selection['tools']['python']['sha256'] = '0' * 64
        r = self.run_launcher('identity')
        self.assertEqual(r.returncode, 2)
        self.assertIn('identity', r.stderr.lower())

    def test_missing_tool_refuses(self):
        Path(self.tools['rustc']['path']).unlink()
        r = self.run_launcher('identity')
        self.assertEqual(r.returncode, 2)

    def test_changed_tool_version_refuses(self):
        # Rebind all content hashes, but leave the expected version incorrect.
        self.tools['cargo']['version'] = 'cargo wrong'
        self.manifest['lanes'][0]['cargo'] = self.tools['cargo']
        self.manifest_path.write_text(json.dumps(self.manifest))
        self.selection['qualification_manifest'] = identity(self.manifest_path)
        self.receipt['manifest_sha256'] = hashlib.sha256(json.dumps(self.manifest, sort_keys=True).encode()).hexdigest()
        self.receipt['tools']['cargo']['version'] = 'cargo wrong'
        self.receipt_path.write_text(json.dumps(self.receipt))
        self.selection['qualification_receipt'] = identity(self.receipt_path)
        r = self.run_launcher('identity')
        self.assertEqual(r.returncode, 2)
        self.assertIn('version mismatch', r.stderr)

    def test_changed_review_refuses(self):
        self.review.write_text('changed\n')
        self.assertEqual(self.run_launcher('identity').returncode, 2)

    def test_failed_qualification_refuses(self):
        self.receipt['status'] = 'Failed'
        self.receipt_path.write_text(json.dumps(self.receipt))
        self.selection['qualification_receipt'] = identity(self.receipt_path)
        self.assertEqual(self.run_launcher('identity').returncode, 2)

    def test_editable_source_is_recorded_without_inheriting_qualification(self):
        (self.repo / 'Cargo.lock').write_text('edited lock\n')
        (self.repo / 'new.txt').write_text('new\n')
        r = self.run_launcher('identity')
        self.assertEqual(r.returncode, 0, r.stderr)
        source = self.latest()['source']
        self.assertTrue(source['dirty'])
        self.assertIn('new.txt', source['untracked'])
        self.assertEqual(source['commit'], self.commit)
        self.assertEqual(source['lock_sha256'], identity(self.repo / 'Cargo.lock')['sha256'])
        self.assertEqual(source['qualification'], 'not assessed')

    def test_cargo_routes_through_wrapper_with_budget(self):
        r = self.run_launcher('--budget-gib', '2', 'cargo', 'test', '--locked')
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout), ['--budget-gib', '2', 'test', '--locked'])
        self.assertEqual(self.latest()['command'][0], str(self.wrapper))

    def test_admission_failure_is_propagated(self):
        r = self.run_launcher('--budget-gib', '2', 'cargo', 'deny')
        self.assertEqual(r.returncode, 17, r.stderr)
        self.assertEqual(self.latest()['exit'], 17)

    def test_cargo_requires_budget(self):
        self.assertEqual(self.run_launcher('cargo', 'test').returncode, 2)

    def test_inherited_runtime_overrides_refuse(self):
        for key in ['RUSTUP_TOOLCHAIN', 'RUSTC', 'PYTHONPATH', 'CARGO_TARGET_DIR', 'LD_PRELOAD', 'CARGO_PROFILE_RELEASE_DEBUG']:
            with self.subTest(key=key):
                r = self.run_launcher('identity', env=dict(self.env, **{key: 'unexpected'}))
                self.assertEqual(r.returncode, 2, r.stderr)

    def test_cargo_command_overrides_refuse(self):
        for args in [('+nightly', 'test'), ('test', '--config=build.rustc="bad"'), ('test', '--target-dir', '/tmp/bad')]:
            with self.subTest(args=args):
                self.assertEqual(self.run_launcher('--budget-gib', '2', 'cargo', *args).returncode, 2)

    def test_cargo_config_refuses(self):
        (self.repo / '.cargo').mkdir()
        (self.repo / '.cargo/config.toml').write_text('[build]\nrustc="unexpected"\n')
        self.assertEqual(self.run_launcher('--budget-gib', '2', 'cargo', 'test').returncode, 2)

    def test_child_command_exit_is_preserved(self):
        r = self.run_launcher('python', '-c', 'raise SystemExit(7)')
        self.assertEqual(r.returncode, 7, r.stderr)
        self.assertEqual(self.latest()['exit'], 7)


if __name__ == '__main__':
    unittest.main()
