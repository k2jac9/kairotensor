"""Behavioral checks use tiny executables, never qualification evidence."""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('matrix', Path(__file__).with_name('verify_runtime_matrix.py'))


class RuntimeMatrixTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(Path(SPEC.origin).exists(), 'runtime verifier must exist')
        self.m = importlib.util.module_from_spec(SPEC)
        SPEC.loader.exec_module(self.m)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.tool = self.root / 'fake'
        self.tool.write_text(f'#!{sys.executable}\nimport sys\nprint("fixture version 1")\n')
        self.tool.chmod(0o755)
        self.identity = {'path': str(self.tool), 'sha256': self.sha(self.tool),
                         'version_args': ['--version'], 'version': 'fixture version 1'}

    @staticmethod
    def sha(p):
        return hashlib.sha256(Path(p).read_bytes()).hexdigest()

    def command(self, code, kind='python', contains=None):
        p = self.root / 'command'
        p.write_text(f'#!{sys.executable}\n{code}\n'); p.chmod(0o755)
        return {'id': 'test', 'argv': [str(p)], 'kind': kind, 'contains': contains or [],
                'timeout': 5, 'artifacts': []}

    def run_command(self, command):
        return self.m.execute_command(command, self.root, os.environ.copy(), self.root / 'logs')

    def test_wrong_binary_digest_refused(self):
        self.identity['sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'digest'):
            self.m.check_identity(self.identity)

    def test_wrong_executable_version_refused(self):
        self.identity['version'] = 'different'
        with self.assertRaisesRegex(ValueError, 'version'):
            self.m.check_identity(self.identity)

    def test_missing_executable_is_unavailable(self):
        self.identity['path'] = str(self.root / 'absent')
        with self.assertRaises(FileNotFoundError):
            self.m.check_identity(self.identity)

    def test_exact_executable_is_executed(self):
        self.assertEqual(self.m.check_identity(self.identity)['version'], 'fixture version 1')

    def test_relative_runtime_refused(self):
        self.identity['path'] = 'fake'
        with self.assertRaisesRegex(ValueError, 'absolute'):
            self.m.check_identity(self.identity)

    def test_missing_role_refused(self):
        with self.assertRaisesRegex(ValueError, 'roles'):
            self.m.validate_roles([{'role': 'stable'}])

    def test_two_changed_runtimes_refused(self):
        lanes = []
        for role in ['stable', 'rust-upcoming', 'rust-previous', 'python-upcoming', 'python-previous']:
            lanes.append({'role': role, 'rustc': {'sha256': role if role.startswith('rust-') else 's'},
                          'python': {'sha256': role if role.startswith('python-') else 'p'}})
        self.m.validate_roles(lanes)
        lanes[1]['python']['sha256'] = 'other'
        with self.assertRaisesRegex(ValueError, 'independent'):
            self.m.validate_roles(lanes)

    def test_changed_lock_or_tracked_file_refused(self):
        subprocess.run(['git','init','-q',str(self.root)], check=True)
        (self.root/'Cargo.lock').write_text('original')
        subprocess.run(['git','-C',str(self.root),'add','.'],check=True)
        subprocess.run(['git','-C',str(self.root),'-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','fixture'],check=True)
        commit=subprocess.check_output(['git','-C',str(self.root),'rev-parse','HEAD'],text=True).strip()
        files={p:self.sha(self.root/p) for p in ['Cargo.lock','fake']}
        source={'commit':commit,'files':files,'lock_sha256':files['Cargo.lock']}
        self.m.check_source(self.root,source)
        (self.root/'Cargo.lock').write_text('changed')
        with self.assertRaises(ValueError): self.m.check_source(self.root,source)

    def test_nonzero_exit_cannot_pass(self):
        r=self.run_command(self.command('print("Ran 2 tests\\nOK"); raise SystemExit(7)'))
        self.assertEqual(r['status'],'Failed'); self.assertEqual(r['exit'],7)

    def test_python_skip_cannot_pass(self):
        r=self.run_command(self.command('print("Ran 2 tests\\nOK (skipped=1)")'))
        self.assertEqual(r['status'],'Failed'); self.assertEqual(r['skips'],1)

    def test_rust_filter_cannot_pass(self):
        r=self.run_command(self.command('print("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out")','rust'))
        self.assertEqual(r['status'],'Failed')

    def test_zero_or_missing_test_count_cannot_pass(self):
        for output in ['Ran 0 tests\\nOK','all fine']:
            r=self.run_command(self.command(f'print({output!r})'))
            self.assertEqual(r['status'],'Failed')

    def test_output_predicate_required(self):
        r=self.run_command(self.command('print("wrong")','fixture',['expected']))
        self.assertEqual(r['status'],'Failed')

    def test_missing_artifact_cannot_pass(self):
        c=self.command('print("expected")','fixture',['expected']);c['artifacts']=[str(self.root/'missing.json')]
        self.assertEqual(self.run_command(c)['status'],'Failed')

    def test_timeout_is_failed_and_retained(self):
        c=self.command('import time; print("started",flush=True); time.sleep(5)');c['timeout']=0.05
        r=self.run_command(c)
        self.assertEqual(r['status'],'Failed');self.assertTrue(r['timeout'])
        self.assertTrue(Path(r['stdout']['path']).is_file())

    def test_success_binds_real_logs_and_counts(self):
        r=self.run_command(self.command('print("Ran 2 tests\\nOK")'))
        self.assertEqual(r['status'],'Pass');self.assertEqual(r['tests'],2)
        self.assertEqual(r['stdout']['sha256'],self.sha(r['stdout']['path']))

    def test_receipt_checks_logs_not_reported_status(self):
        c=self.command('print("Ran 2 tests\\nOK")');r=self.run_command(c)
        self.m.check_command_receipt(c,r)
        Path(r['stdout']['path']).write_text('tampered')
        with self.assertRaises(ValueError):self.m.check_command_receipt(c,r)

    def test_missing_receipt_refused(self):
        with self.assertRaisesRegex(ValueError,'receipt'):
            self.m.check_command_receipt(self.command('pass'),{})

    def test_forged_pass_with_skip_rejected(self):
        c=self.command('print("Ran 2 tests\\nOK (skipped=1)")');r=self.run_command(c);r['status']='Pass'
        with self.assertRaises(ValueError):self.m.check_command_receipt(c,r)

    def test_private_alias_selects_child_python(self):
        lane={'rustc':self.identity,'cargo':self.identity,'python':self.identity}
        env=self.m.lane_environment(lane,self.root/'aliases',{}, {'PATH':os.environ.get('PATH','')})
        r=subprocess.run(['python3','--version'],env=env,text=True,capture_output=True,check=True)
        self.assertEqual(r.stdout.strip(),'fixture version 1')
        self.assertEqual(Path(env['PATH'].split(os.pathsep)[0],'python3').resolve(),self.tool)

    def test_inherited_compiler_override_refused(self):
        lane={'rustc':self.identity,'cargo':self.identity,'python':self.identity}
        with self.assertRaisesRegex(ValueError,'override'):
            self.m.lane_environment(lane,self.root/'aliases',{},dict(os.environ,RUSTC_WRAPPER='/wrong'))

    def test_failed_unittest_text_cannot_be_forged_as_ok(self):
        r=self.run_command(self.command('print("Ran 2 tests\\nFAILED (failures=1)\\nOK")'))
        self.assertEqual(r['status'],'Failed')

    def test_unknown_skip_text_cannot_pass(self):
        r=self.run_command(self.command('print("Ran 2 tests\\nOK\\nSKIP required runner unavailable")'))
        self.assertEqual(r['status'],'Failed')

    def test_native_override_refused(self):
        lane={'rustc':self.identity,'cargo':self.identity,'python':self.identity}
        with self.assertRaisesRegex(ValueError,'override'):
            self.m.lane_environment(lane,self.root/'aliases',{}, {'PATH':os.environ.get('PATH',''),'CC':'/other/cc'})

    def manifest(self):
        repo=self.root/'repo';repo.mkdir()
        (repo/'Cargo.lock').write_text('fixture lock')
        subprocess.run(['git','init','-q',str(repo)],check=True)
        subprocess.run(['git','-C',str(repo),'add','.'],check=True)
        subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','fixture'],check=True)
        commit=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()
        identity={'path':str(self.tool),'sha256':self.sha(self.tool)}
        lanes=[{'role':role,'checkout':str(repo),'rustc':self.identity,'cargo':self.identity,
                'python':self.identity,'provenance':[identity]}
               for role in ['stable','rust-upcoming','rust-previous','python-upcoming','python-previous']]
        return {'schema':'kairotensor-runtime-matrix/1','host_path':os.environ['PATH'],'lanes':lanes,
                'source':{'commit':commit,'files':{'Cargo.lock':self.sha(repo/'Cargo.lock')},'lock_sha256':self.sha(repo/'Cargo.lock')},
                'commands':self.m.command_inventory(),'native_files':[identity],
                **{k:identity for k in ['wrapper','storage_guard','cargo_recipe','bundle','attestations']}}

    def test_complete_manifest_rejects_omitted_command(self):
        m=self.manifest();m['commands'].pop()
        with self.assertRaisesRegex(ValueError,'inventory'): self.m.check_manifest(m)

    def test_unavailable_lane_emits_no_pass_or_commands(self):
        m=self.manifest();lane=copy.deepcopy(m['lanes'][0]);lane['cargo']['path']=str(self.root/'unavailable')
        r=self.m.run_lane(m,lane,self.root/'result')
        self.assertEqual(r['status'],'Unexecuted');self.assertEqual(r['commands'],[])
        self.assertTrue((self.root/'result'/'receipt.json').is_file())

    def test_receipt_binds_whole_manifest(self):
        m=self.manifest();receipt={'role':'stable','source':m['source'],'status':'Pass','manifest_sha256':'0'*64}
        with self.assertRaisesRegex(ValueError,'manifest'):
            self.m.verify_receipt(m,m['lanes'][0],receipt)

    def test_directory_artifact_retains_empty_log_with_nonempty_result(self):
        d=self.root/'outputs';d.mkdir();(d/'result.json').write_text('{"status":"complete"}');(d/'stderr.log').touch()
        records=self.m.output_files([str(d)])
        self.assertEqual({r['path'] for r in records},{str(d/'result.json'),str(d/'stderr.log')})
        self.assertEqual(next(r['sha256'] for r in records if r['path'].endswith('stderr.log')),
                         'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855')

    def test_all_empty_directory_artifact_refused(self):
        d=self.root/'outputs';d.mkdir();(d/'stderr.log').touch()
        with self.assertRaisesRegex(ValueError,'empty'): self.m.output_files([str(d)])

    def test_explicit_empty_result_file_refused(self):
        p=self.root/'result.json';p.touch()
        with self.assertRaisesRegex(ValueError,'empty'): self.m.output_files([str(p)])

    def test_rust_receipt_survives_json_roundtrip(self):
        c=self.command('print("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")','rust')
        r=json.loads(json.dumps(self.run_command(c)))
        self.m.check_command_receipt(c,r)
        self.assertEqual(r['tests'],2)

    def test_json_rust_receipt_tampered_count_rejected(self):
        c=self.command('print("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")','rust')
        r=json.loads(json.dumps(self.run_command(c)));r['tests']=200
        with self.assertRaises(ValueError):self.m.check_command_receipt(c,r)

    def test_json_rust_receipt_forged_pass_on_ignore_rejected(self):
        c=self.command('print("test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out")','rust')
        r=json.loads(json.dumps(self.run_command(c)));r['status']='Pass';r['skips']=0
        with self.assertRaises(ValueError):self.m.check_command_receipt(c,r)

    def context_fixture(self):
        lane={'checkout':str(self.root),'rustc':self.identity,'cargo':self.identity,'python':self.identity}
        manifest={'host_path':'/usr/bin:/bin'}
        env={'CARGO_PROFILE_DEV_DEBUG':'0','CARGO_PROFILE_TEST_DEBUG':'0','CARGO_INCREMENTAL':'0',
             'KAIROTENSOR_REQUIRE_LINUX_CLOCK':'1','RUSTUP_AUTO_INSTALL':'0','UV_PYTHON_DOWNLOADS':'never',
             'PYTHONDONTWRITEBYTECODE':'1','PATH':str(self.root/'bin')+':/usr/bin:/bin',
             'RUSTC':str(self.tool),'RUNNER_TEMP':str(self.root/'outputs')}
        return manifest,lane,{'cwd':str(self.root),'environment':env}

    def test_recorded_context_roundtrip(self):
        m,lane,r=self.context_fixture()
        self.m.check_command_context(m,lane,self.root,json.loads(json.dumps(r)))

    def test_contradictory_cwd_rejected(self):
        m,lane,r=self.context_fixture();r['cwd']='/another/checkout'
        with self.assertRaisesRegex(ValueError,'context'):self.m.check_command_context(m,lane,self.root,r)

    def test_contradictory_runtime_profile_or_path_rejected(self):
        for key,value in [('RUSTC','/wrong/rustc'),('CARGO_PROFILE_DEV_DEBUG','2'),('PATH','/usr/bin'),('RUNNER_TEMP','/wrong/output')]:
            with self.subTest(key=key):
                m,lane,r=self.context_fixture();r['environment'][key]=value
                with self.assertRaisesRegex(ValueError,'context'):self.m.check_command_context(m,lane,self.root,r)

    def test_missing_or_wrong_runner_environment_rejected(self):
        m,lane,r=self.context_fixture();runners={k:self.identity for k in ['recovery_policy_eval','decision_replay','policy_activation']}
        with self.assertRaisesRegex(ValueError,'context'):self.m.check_command_context(m,lane,self.root,r,runners)
        r['environment'].update(KAIROTENSOR_RECOVERY_RUNNER=str(self.tool),KAIROTENSOR_DECISION_REPLAY='/wrong/runner')
        with self.assertRaisesRegex(ValueError,'context'):self.m.check_command_context(m,lane,self.root,r,runners)

    def test_alias_retargeting_rejected(self):
        lane={'rustc':self.identity,'cargo':self.identity,'python':self.identity}
        aliases=self.root/'aliases';aliases.mkdir()
        for name in ['rustc','cargo','python3']:(aliases/name).symlink_to(self.tool)
        self.m.check_aliases(lane,aliases)
        (aliases/'python3').unlink();(aliases/'python3').symlink_to(sys.executable)
        with self.assertRaisesRegex(ValueError,'alias'):self.m.check_aliases(lane,aliases)

    def test_host_path_must_have_nonempty_absolute_components(self):
        m=self.manifest()
        for path in ['', '/bin::/usr/bin','relative:/bin']:
            m['host_path']=path
            with self.assertRaisesRegex(ValueError,'host_path'):self.m.check_manifest(m)

    def test_structured_actual_python_parent_child_probe_roundtrip(self):
        identity={'path':sys.executable,'sha256':self.sha(sys.executable),'version_args':['--version'],
                  'version':subprocess.check_output([sys.executable,'--version'],text=True).strip()}
        lane={'python':identity,'rustc':identity,'cargo':identity}
        aliases=self.root/'aliases';env=self.m.lane_environment(lane,aliases,{}, {'PATH':os.environ['PATH']})
        probe=json.loads(json.dumps(self.m.probe_python(lane,env)))
        self.m.check_python_probe(lane,aliases,probe)
        self.assertEqual(probe['observed']['parent']['realpath'],str(Path(sys.executable).resolve()))
        self.assertEqual(probe['observed']['child']['realpath'],str(Path(sys.executable).resolve()))
        probe['observed']['child']['realpath']='/wrong/python'
        with self.assertRaisesRegex(ValueError,'probe'):self.m.check_python_probe(lane,aliases,probe)

    def test_compiler_artifact_runner_paths_are_authoritative(self):
        log=self.root/'compiler.stdout'
        log.write_text('\n'.join(json.dumps({'reason':'compiler-artifact','target':{'name':name},'executable':str(self.tool)})
                                  for name in ['recovery_policy_eval','decision_replay','policy_activation']))
        r={'stdout':{'path':str(log),'sha256':self.sha(log)}}
        expected={name:{'path':str(self.tool),'sha256':self.sha(self.tool)}
                  for name in ['recovery_policy_eval','decision_replay','policy_activation']}
        self.assertEqual(self.m.compiled_runners(r),expected)
        log.write_text('{}')
        with self.assertRaises(ValueError):self.m.compiled_runners(r)

    def test_inventory_contains_all_distinct_commands(self):
        commands=self.m.command_inventory()
        ids=[c['id'] for c in commands]
        self.assertEqual(len(ids),len(set(ids)))
        self.assertTrue({'all-targets','all-python','compile-required-runners','ablation-plan','ablation-score','verifier-unit'} <= set(ids))


if __name__ == '__main__':
    unittest.main()
