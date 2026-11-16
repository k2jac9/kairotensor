#!/usr/bin/env python3
"""Run a frozen offline workload with explicitly identified runtimes.

No installation, mutable runtime lookup, or activation is performed. Manifests
and receipts are local evidence, not signatures or independent acceptance.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import time

ROLES = {'stable', 'rust-upcoming', 'rust-previous', 'python-upcoming', 'python-previous'}
OVERRIDES = {'RUSTC', 'RUSTDOC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
             'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTC',
             'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTFLAGS', 'CARGO_TARGET_DIR',
             'CC', 'CXX', 'AR', 'LD', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'RUSTUP_TOOLCHAIN',
             'PYTHONHOME', 'PYTHONPATH', 'PYTHONSTARTUP', 'VIRTUAL_ENV', 'CONDA_PREFIX'}
PROFILE = {'CARGO_PROFILE_DEV_DEBUG': '0', 'CARGO_PROFILE_TEST_DEBUG': '0',
           'CARGO_INCREMENTAL': '0', 'KAIROTENSOR_REQUIRE_LINUX_CLOCK': '1',
           'RUSTUP_AUTO_INSTALL': '0', 'UV_PYTHON_DOWNLOADS': 'never',
           'PYTHONDONTWRITEBYTECODE': '1'}


def sha(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def file_identity(path):
    return {'path': str(Path(path).absolute()), 'sha256': sha(path)}


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def check_file(identity):
    p = Path(identity['path'])
    if not p.is_absolute():
        raise ValueError('identity requires absolute path')
    if sha(p) != identity['sha256']:
        raise ValueError(f'digest mismatch: {p}')
    return p


def check_identity(identity):
    p = check_file(identity)
    if not os.access(p, os.X_OK):
        raise ValueError(f'not executable: {p}')
    r = subprocess.run([str(p), *identity['version_args']], capture_output=True,
                       text=True, timeout=20, check=True)
    version = r.stdout.strip()
    if version != identity['version']:
        raise ValueError(f'version mismatch: {p}: {version!r}')
    return {**file_identity(p), 'resolved_path': str(p.resolve()), 'version': version}


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def check_source(root, source):
    root = Path(root)
    if git(root, 'rev-parse', 'HEAD') != source['commit']:
        raise ValueError('source commit mismatch')
    names = git(root, 'ls-files').splitlines()
    if set(names) != set(source['files']):
        raise ValueError('incomplete tracked source manifest')
    for name, digest in source['files'].items():
        if sha(root / name) != digest:
            raise ValueError(f'changed source: {name}')
    if sha(root / 'Cargo.lock') != source['lock_sha256']:
        raise ValueError('changed lock')
    if git(root, 'status', '--porcelain', '--untracked-files=normal'):
        raise ValueError('dirty source')
    # Bind hashes to the commit too, not merely to whatever is on disk.
    for name, digest in source['files'].items():
        data = subprocess.check_output(['git', '-C', str(root), 'show', f'{source["commit"]}:{name}'])
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f'source manifest differs from commit: {name}')


def validate_roles(lanes):
    if len(lanes) != 5 or {l['role'] for l in lanes} != ROLES:
        raise ValueError('exactly five distinct roles required')
    stable = next(l for l in lanes if l['role'] == 'stable')
    for lane in lanes:
        role = lane['role']
        fixed = 'python' if role.startswith('rust-') else 'rustc'
        if role != 'stable' and lane[fixed]['sha256'] != stable[fixed]['sha256']:
            raise ValueError(f'independent runtime variation required: {role}')


def lane_environment(lane, aliases, explicit, inherited=None):
    env = dict(os.environ if inherited is None else inherited)
    unexpected = sorted(k for k in OVERRIDES if env.get(k))
    unexpected += sorted(k for k in env if (k.startswith('CARGO_TARGET_') or k.startswith('CARGO_PROFILE_'))
                         and env[k] and (k not in PROFILE or env[k] != PROFILE[k]))
    if unexpected:
        raise ValueError(f'unbound inherited override: {unexpected}')
    aliases = Path(aliases)
    aliases.mkdir(exist_ok=False)
    for alias, key in [('rustc', 'rustc'), ('cargo', 'cargo'), ('python3', 'python')]:
        check_identity(lane[key])
        (aliases / alias).symlink_to(lane[key]['path'])
    env.update(PROFILE)
    env.update(explicit)
    env['PATH'] = str(aliases) + os.pathsep + env.get('PATH', '')
    env['RUSTC'] = lane['rustc']['path']
    return env



def check_aliases(lane, aliases):
    for alias, key in [('rustc','rustc'),('cargo','cargo'),('python3','python')]:
        path=Path(aliases)/alias
        if not path.is_symlink() or path.resolve()!=Path(lane[key]['path']).resolve():
            raise ValueError(f'private alias mismatch: {alias}')
        check_file({'path':str(path),'sha256':lane[key]['sha256']})


def check_command_context(m,lane,out,record,runners=None):
    expected={**PROFILE,'PATH':str(Path(out)/'bin')+os.pathsep+m['host_path'],
              'RUSTC':lane['rustc']['path'],'RUNNER_TEMP':str(Path(out)/'outputs')}
    if runners is not None:
        expected.update(KAIROTENSOR_RECOVERY_RUNNER=runners['recovery_policy_eval']['path'],
                        KAIROTENSOR_DECISION_REPLAY=runners['decision_replay']['path'])
    if record.get('cwd')!=lane['checkout'] or record.get('environment')!=expected:
        raise ValueError('command context contradicts declared checkout/runtime environment')


PYTHON_PROBE_CODE = """import sys,os,json,subprocess,sqlite3,ssl,ctypes,zlib,bz2,lzma
child=json.loads(subprocess.check_output(['python3','-c',
'import sys,os,json;print(json.dumps({"realpath":os.path.realpath(sys.executable),"version":"Python "+sys.version.split()[0]}))'],text=True))
print(json.dumps({'parent':{'realpath':os.path.realpath(sys.executable),'version':'Python '+sys.version.split()[0]},'child':child}))
"""


def probe_python(lane,env):
    argv=[lane['python']['path'],'-c',PYTHON_PROBE_CODE]
    r=subprocess.run(argv,env=env,capture_output=True,text=True,timeout=20)
    probe={'argv':argv,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    try: probe['observed']=json.loads(r.stdout)
    except ValueError: probe['observed']=None
    return probe


def check_python_probe(lane,aliases,probe):
    check_aliases(lane,aliases)
    expected={'realpath':str(Path(lane['python']['path']).resolve()),'version':lane['python']['version']}
    try: observed=json.loads(probe['stdout'])
    except (KeyError,ValueError,TypeError):raise ValueError('invalid Python probe output') from None
    if (probe.get('exit')!=0 or probe.get('argv')!=[lane['python']['path'],'-c',PYTHON_PROBE_CODE]
            or observed!={'parent':expected,'child':expected} or probe.get('observed')!=observed
            or not isinstance(probe.get('stderr'),str)):
        raise ValueError('Python probe contradicts selected parent/child interpreter')


def compiled_runners(record):
    check_file(record['stdout'])
    required={'recovery_policy_eval','decision_replay','policy_activation'}
    runners={}
    for line in Path(record['stdout']['path']).read_text().splitlines():
        try: row=json.loads(line)
        except ValueError: continue
        if row.get('reason')!='compiler-artifact' or not row.get('executable'):continue
        name=row['target']['name']
        if name not in required:continue
        if not Path(row['executable']).is_absolute():raise ValueError('runner path must be absolute')
        identity=file_identity(row['executable'])
        if name in runners and runners[name]!=identity:raise ValueError('conflicting compiled runner paths')
        runners[name]=identity
    if set(runners)!=required:raise ValueError('missing compiled runner artifacts')
    return runners

def command_inventory():
    """Source check.yml workload plus E1 demonstrations; placeholders are explicit."""
    commands = []
    def add(name, argv, kind='fixture', contains=(), artifacts=(), stdout=None):
        c = dict(id=name, argv=argv, kind=kind, contains=list(contains),
                 artifacts=list(artifacts), timeout=600 if argv[0] == '{wrapper}' else 300)
        if stdout: c['stdout_path'] = stdout
        commands.append(c)
    cargo = ['{wrapper}', '--budget-gib', '2']
    py = ['{python}']
    add('compile-required-runners', cargo + ['build', '--locked', '--offline', '--example',
        'recovery_policy_eval', '--example', 'decision_replay', '--example', 'policy_activation',
        '--message-format=json'], contains=['compiler-artifact'])
    add('all-targets', cargo + ['test', '--locked', '--all-targets'], 'rust')
    examples = [('withdrawal','Rebuilt stock count'), ('deterministic_verdict','current_read'),
        ('deadline_aware_recovery','synthetic_deadline_admission_fixture'),
        ('runtime_recovery','claimed_hold_retained'), ('selective_revision','Published attempt'),
        ('corrected_handoff','checklist_passed'), ('scoped_model_reuse','reuse_hits'),
        ('token_cap_eval','boundary'), ('budgeted_recovery','deterministic_byte_token_fixture')]
    for name, predicate in examples:
        add(name,cargo+['run','--locked','--example',name],contains=[predicate])
        if name == 'scoped_model_reuse':
            add('scoped-model-replay-invalid',cargo+['run','--locked','--example',name,'--','--replay-invalid'],contains=['invalid_output'])
    add('reference',py+['-m','unittest','discover','-s','reference','-p','test_memory.py'],'python')
    add('oracle',py+['reference/belief_oracle.py'],'python')
    add('all-python',py+['-m','unittest','discover','-s','scripts','-p','test_*.py'],'python')
    for name, pattern in [('ablation-unit','test_decision_ablation.py'),('verifier-unit','test_verify_runtime_matrix.py')]:
        add(name,py+['-m','unittest','discover','-s','scripts','-p',pattern],'python')
    for action in ['export-fixture','replay']:
        add('decision-'+action,cargo+['run','--locked','--offline','--example','decision_replay','--',action,'{outputs}/decision-v1.json'],contains=['matched'],artifacts=['{outputs}/decision-v1.json'])
    add('decision-audit',py+['scripts/audit_recovery_bundle.py','--decision-v1','{outputs}/decision-v1.json'],contains=['integrity_checked'])
    add('token-cap-archives',py+['scripts/summarize_token_caps.py']+[
        f'docs/evaluations/token-caps-{n}-2026-11-13.json' for n in ['compatibility','integration','migration','package','unit']],contains=['Calibration'])
    add('synthetic-comparison',py+['scripts/recovery_policy_eval.py','--fixture'],contains=['synthetic_policy_fixture'])
    add('activation-fixture-input',py+['scripts/recovery_policy_eval.py','--activation-fixture'],contains=['policy-activation-fixture/1'],stdout='{outputs}/policy-activation-fixture.json',artifacts=['{outputs}/policy-activation-fixture.json'])
    add('policy-activation',cargo+['run','--locked','--offline','--example','policy_activation','--','--fixture','{outputs}/policy-activation-fixture.json','--out','{outputs}/policy-activation'],contains=['baseline_manifest'],artifacts=['{outputs}/policy-activation'])
    add('archived-v1-refusal',py+['scripts/recovery_policy_eval.py','--archived-v1-refusal'],contains=['refus'],stdout='{outputs}/archived-v1-promotion-refusal.json',artifacts=['{outputs}/archived-v1-promotion-refusal.json'])
    add('legacy-collector',cargo+['run','--locked','--example','recovery_policy_eval','--','--trace','{outputs}/recovery-policy-trace.json'],contains=['complete_policy_trace'],artifacts=['{outputs}/recovery-policy-trace.json'])
    add('legacy-score',py+['scripts/recovery_policy_eval.py','{outputs}/recovery-policy-trace.json'],contains=['synthetic_policy_fixture'])
    for name, pattern in [('explicit-recovery-runner','test_recovery_policy_runner.py'),('explicit-audit-runner','test_audit_recovery_bundle.py')]:
        add(name,py+['-m','unittest','discover','-s','scripts','-p',pattern],'python')
    add('calibration',py+['scripts/recovery_calibration.py','--output','{outputs}/recovery-calibration.json'],contains=['complete'],artifacts=['{outputs}/recovery-calibration.json'])
    add('timed-runtime',cargo+['run','--locked','--example','timed_recovery_runner','--','--out','{outputs}/timed-recovery-runner'],contains=['accounting_reconciled'],artifacts=['{outputs}/timed-recovery-runner'])
    add('incident-prepare',py+['scripts/incident_routing.py','prepare','--out','{outputs}/incident-routing-calibration'],contains=['offline_preparation'],artifacts=['{outputs}/incident-routing-calibration'])
    add('incident-bridge',cargo+['run','--locked','--example','incident_store_bridge','--','--requests','{outputs}/incident-store-requests.json'],artifacts=['{outputs}/incident-store-requests.json'])
    add('incident-bridge-check',py+['scripts/check_incident_store_bridge.py','--requests','{outputs}/incident-store-requests.json'],contains=['offline_store_byte_parity'])
    add('explicit-incident-bridge-tests',py+['-m','unittest','discover','-s','scripts','-p','test_incident_store_bridge.py'],'python')
    add('incident-preflight',py+['scripts/incident_preflight.py','--out','{outputs}/incident-preflight-plan.json'],contains=['offline_calibration_plan'],artifacts=['{outputs}/incident-preflight-plan.json'])
    add('incident-controller',py+['scripts/incident_calibration_controller.py','fixture','--out','{outputs}/incident-controller-fixture'],contains=['offline_calibration_diagnostic'],artifacts=['{outputs}/incident-controller-fixture'])
    add('incident-transformers',py+['scripts/incident_transformers_controller.py','fixture','--out','{outputs}/incident-transformers-fixture'],contains=['offline_fake_dependencies'],artifacts=['{outputs}/incident-transformers-fixture'])
    add('ablation-plan',py+['scripts/decision_ablation.py','plan','{bundle}'],contains=['decision-ablation-plan/1'],stdout='{outputs}/ablation-plan.json',artifacts=['{outputs}/ablation-plan.json'])
    add('ablation-score',py+['scripts/decision_ablation.py','score','{outputs}/ablation-plan.json','{attestations}'],contains=['Comparable','Complete','fixture_bytes'],artifacts=[])
    return commands


def render(value, variables):
    if isinstance(value,str):
        for key, replacement in variables.items(): value=value.replace('{'+key+'}',str(replacement))
        return value
    if isinstance(value,list): return [render(v,variables) for v in value]
    if isinstance(value,dict): return {k:render(v,variables) for k,v in value.items()}
    return value


def assess(command, record, raw):
    counts = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out', raw)
    py = re.findall(r'^Ran (\d+) tests?\b', raw, re.M)
    skips = sum(int(r[3])+int(r[5]) for r in counts)
    skips += sum(map(int,re.findall(r'skipped=(\d+)',raw)))
    failed = sum(int(r[2]) for r in counts)
    failed += len(re.findall(r'^FAILED\b',raw,re.M))
    skips += len(re.findall(r'^.*\b(?:SKIP|skipped)\b(?![=])[^\n]*$',raw,re.M))
    tests = sum(int(r[1]) for r in counts) if command['kind']=='rust' else sum(map(int,py))
    reasons=[]
    if record['exit'] != 0: reasons.append('nonzero exit')
    if record['timeout']: reasons.append('timeout')
    if skips: reasons.append('skipped/ignored/filtered checks')
    if failed: reasons.append('failed tests')
    if command['kind'] in ('rust','python') and tests==0: reasons.append('missing/zero tests')
    if command['kind']=='python' and not re.search(r'^OK\s*$',raw,re.M): reasons.append('missing unittest success')
    if any(s not in raw for s in command['contains']): reasons.append('missing output predicate')
    return dict(status='Failed' if reasons else 'Pass', reasons=reasons, tests=tests,
                skips=skips, rust_results=[list(row) for row in counts], python_counts=py)


def output_files(paths):
    files=[]
    for name in paths:
        p=Path(name)
        if not p.exists(): raise ValueError(f'missing artifact: {p}')
        children=sorted(p.rglob('*')) if p.is_dir() else [p]
        found=[c for c in children if c.is_file()]
        if not found or not any(c.stat().st_size>0 for c in found): raise ValueError(f'empty artifact: {p}')
        files.extend(file_identity(c) for c in found)
    return files


def execute_command(command, root, env, logs):
    logs=Path(logs);logs.mkdir(parents=True,exist_ok=True)
    stdout=Path(command.get('stdout_path',logs/(command['id']+'.stdout')))
    stderr=logs/(command['id']+'.stderr')
    start=time.monotonic()
    record={'id':command['id'],'argv':command['argv'],'cwd':str(root),
            'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'environment':{k:env[k] for k in [*PROFILE,'PATH','RUSTC','RUNNER_TEMP','KAIROTENSOR_RECOVERY_RUNNER','KAIROTENSOR_DECISION_REPLAY'] if k in env},
            'timeout':False,'exit':None}
    # Claim both outputs before dispatch. A prior file or symlink is evidence,
    # never permission to truncate it. Keep any partial claim after a refusal.
    with stdout.open('xb') as out,stderr.open('xb') as err:
        try:
            p=subprocess.Popen(command['argv'],cwd=root,env=env,stdout=out,stderr=err,start_new_session=True)
            try: record['exit']=p.wait(timeout=command['timeout'])
            except subprocess.TimeoutExpired:
                os.killpg(p.pid,signal.SIGKILL);record['exit']=p.wait();record['timeout']=True
        except OSError as e: err.write(str(e).encode())
    record.update(elapsed_seconds=time.monotonic()-start,stdout=file_identity(stdout),stderr=file_identity(stderr))
    raw=stdout.read_text(errors='replace')+'\n'+stderr.read_text(errors='replace')
    record.update(assess(command,record,raw))
    record['storage_reservations']=re.findall(r'Storage reservation: ([a-f0-9-]+)',raw)
    try: record['artifacts']=output_files(command['artifacts'])
    except ValueError as e: record.update(status='Failed');record['reasons'].append(str(e));record['artifacts']=[]
    return record


def check_command_receipt(command, record):
    if not record or record.get('id') != command['id'] or record.get('argv') != command['argv']:
        raise ValueError('missing/mismatched command receipt')
    for key in ['stdout','stderr']: check_file(record[key])
    raw=Path(record['stdout']['path']).read_text(errors='replace')+'\n'+Path(record['stderr']['path']).read_text(errors='replace')
    observed=assess(command,record,raw)
    if observed['status']!='Pass' or any(record.get(k)!=v for k,v in observed.items()):
        raise ValueError('receipt does not prove passing checks')
    actual=output_files(command['artifacts'])
    if actual!=record['artifacts']: raise ValueError('changed/missing receipt artifact')


def check_manifest(m):
    """Check shared inputs and every role's contract, not its availability."""
    if m.get('schema')!='kairotensor-runtime-matrix/1': raise ValueError('unknown manifest schema')
    host_path=m.get('host_path','')
    if not isinstance(host_path,str) or any(not p or not Path(p).is_absolute() for p in host_path.split(os.pathsep)):
        raise ValueError('host_path must contain nonempty absolute directories')
    source=m['source'];files=source['files']
    if (not re.fullmatch(r'[0-9a-f]{40}',source['commit']) or not isinstance(files,dict)
            or files.get('Cargo.lock')!=source['lock_sha256']
            or not re.fullmatch(r'[0-9a-f]{64}',source['lock_sha256'])
            or any(Path(name).is_absolute() or '..' in Path(name).parts
                   or not re.fullmatch(r'[0-9a-f]{64}',digest) for name,digest in files.items())):
        raise ValueError('invalid shared source contract')
    validate_roles(m['lanes'])
    if m['commands'] != command_inventory(): raise ValueError('incomplete or changed required command inventory')
    for field in ['wrapper','storage_guard','cargo_recipe','bundle','attestations']:
        check_file(m[field])
    if not m['native_files']: raise ValueError('missing native identities')
    for identity in m['native_files']: check_file(identity)
    for lane in m['lanes']:
        if not Path(lane['checkout']).is_absolute(): raise ValueError('absolute checkout required')
        if not lane.get('provenance'): raise ValueError('missing runtime archive/closure provenance')
        for identity in [*lane['provenance'],*(lane[k] for k in ['rustc','cargo','python'])]:
            if not Path(identity['path']).is_absolute(): raise ValueError('identity requires absolute path')
            if not re.fullmatch(r'[0-9a-f]{64}',identity['sha256']): raise ValueError('invalid identity digest')
        for key in ['rustc','cargo','python']:
            identity=lane[key]
            if (not isinstance(identity['version'],str) or not identity['version']
                    or not isinstance(identity['version_args'],list)
                    or not all(isinstance(arg,str) for arg in identity['version_args'])):
                raise ValueError('invalid runtime version contract')


def check_lane(m,lane):
    root=Path(lane['checkout'])
    if not root.exists(): raise FileNotFoundError(f'unavailable checkout: {root}')
    check_source(root,m['source'])
    for identity in lane['provenance']: check_file(identity)


def run_lane(m,lane,out,blocked_reason=None):
    out=Path(out);out.mkdir(parents=True,exist_ok=False)
    root=Path(lane['checkout']); outputs=out/'outputs';outputs.mkdir()
    result={'role':lane['role'],'source':m['source'],'status':'Unexecuted','commands':[],
            'host':{'platform':platform.platform(),'machine':platform.machine()},'new_model_calls':0,
            'manifest_sha256':hashlib.sha256(json.dumps(m,sort_keys=True).encode()).hexdigest()}
    save(out/'receipt.json',result)
    try:
        check_manifest(m)
        check_lane(m,lane)
        if blocked_reason:
            result['error']=blocked_reason
            save(out/'receipt.json',result)
            return result
        tools={k:check_identity(lane[k]) for k in ['rustc','cargo','python']}
        result['tools']=tools
        env=lane_environment(lane,out/'bin',{'RUNNER_TEMP':str(outputs)},dict(os.environ,PATH=m['host_path']))
        result['python_child_probe']=probe_python(lane,env)
        save(out/'receipt.json',result)
        check_python_probe(lane,out/'bin',result['python_child_probe'])
        variables={'wrapper':m['wrapper']['path'],'python':lane['python']['path'],
                   'outputs':outputs,'bundle':m['bundle']['path'],'attestations':m['attestations']['path']}
        commands=render(m['commands'],variables);result['resolved_commands']=commands
        for command in commands:
            check_source(root,m['source'])
            for identity in tools.values(): check_file(identity)
            result['status']='Failed'  # started work cannot become Unexecuted
            record=execute_command(command,root,env,out/'logs')
            result['commands'].append(record);save(out/'receipt.json',result)
            print(lane['role'],command['id'],record['status'],record['exit'],flush=True)
            if record['status']!='Pass': break
            if command['id']=='compile-required-runners':
                runners=compiled_runners(record)
                for name,key in [('recovery_policy_eval','KAIROTENSOR_RECOVERY_RUNNER'),('decision_replay','KAIROTENSOR_DECISION_REPLAY')]:
                    env[key]=runners[name]['path']
                result['runners']=runners
        check_manifest(m);check_lane(m,lane);result['source_verified_after']=True
        for identity in tools.values(): check_file(identity)
        for identity in result.get('runners',{}).values(): check_file(identity)
        if len(result['commands'])==len(commands) and all(r['status']=='Pass' for r in result['commands']):
            for c,r in zip(commands,result['commands']): check_command_receipt(c,r)
            result['status']='Pass'
            save(out/'receipt.json',result)
            verify_receipt(m,lane,json.loads((out/'receipt.json').read_text()))
    except (OSError,ValueError,KeyError,subprocess.SubprocessError) as error:
        result['error']=str(error)
        if result['commands'] or not isinstance(error,OSError):result['status']='Failed'
    save(out/'receipt.json',result)
    return result


def verify_receipt(m,lane,receipt):
    check_manifest(m)
    check_lane(m,lane)
    if receipt.get('manifest_sha256')!=hashlib.sha256(json.dumps(m,sort_keys=True).encode()).hexdigest():
        raise ValueError('receipt manifest identity mismatch')
    if receipt.get('role')!=lane['role'] or receipt.get('source')!=m['source'] or receipt.get('status')!='Pass':
        raise ValueError('wrong source/role/status in receipt')
    if not receipt.get('source_verified_after'): raise ValueError('missing source verification')
    for k in ['rustc','cargo','python']:
        if receipt['tools'][k]!=check_identity(lane[k]): raise ValueError('runtime receipt mismatch')
    if not receipt.get('commands'):raise ValueError('missing command receipts')
    runners=compiled_runners(receipt['commands'][0])
    if receipt.get('runners')!=runners:raise ValueError('runner receipt contradicts compiler artifacts')
    out=Path(receipt['commands'][0]['stdout']['path']).parent.parent
    check_python_probe(lane,out/'bin',receipt.get('python_child_probe',{}))
    commands=receipt['resolved_commands']
    variables={'wrapper':m['wrapper']['path'],'python':lane['python']['path'],
        'outputs':str(Path(receipt['commands'][0]['stdout']['path']).parent.parent/'outputs'),
        'bundle':m['bundle']['path'],'attestations':m['attestations']['path']}
    if commands!=render(m['commands'],variables) or len(receipt['commands'])!=len(commands):
        raise ValueError('missing or changed command receipts')
    for index,(c,r) in enumerate(zip(commands,receipt['commands'])):
        check_command_context(m,lane,out,r,None if index==0 else runners)
        check_command_receipt(c,r)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--manifest',type=Path,required=True)
    p.add_argument('--output',type=Path)
    p.add_argument('--lane',choices=sorted(ROLES))
    p.add_argument('--verify-receipt',type=Path)
    args=p.parse_args();m=json.loads(args.manifest.read_text());check_manifest(m)
    if args.verify_receipt:
        if not args.lane: p.error('--verify-receipt requires --lane')
        lane=next(l for l in m['lanes'] if l['role']==args.lane)
        verify_receipt(m,lane,json.loads(args.verify_receipt.read_text()));return 0
    if not args.output: p.error('--output required for execution')
    results=[]
    for lane in m['lanes']:
        if args.lane and lane['role']!=args.lane: continue
        blocked_reason=None
        if lane['role'].startswith('python-'):
            stable=args.output/'stable'/'receipt.json'
            try:
                verify_receipt(m,next(l for l in m['lanes'] if l['role']=='stable'),json.loads(stable.read_text()))
            except (OSError,ValueError,KeyError,subprocess.SubprocessError) as error:
                blocked_reason=f'validated stable qualification required: {error}'
        results.append(run_lane(m,lane,args.output/lane['role'],blocked_reason))
    return 0 if results and all(r['status']=='Pass' for r in results) else 1


if __name__=='__main__':
    try: raise SystemExit(main())
    except (ValueError,OSError,KeyError,subprocess.SubprocessError) as e:
        raise SystemExit(f'Runtime matrix refused: {e}')
