#!/usr/bin/env python3
"""Launch project commands with a locally accepted, explicitly identified runtime.

The trusted owner maintains the selection and acceptance artifacts. Hashes detect
changes; they do not authenticate their issuer. Python programs are not sandboxed.
"""
import argparse
import datetime
import hashlib
import json
import math
import os
import shutil
from pathlib import Path
import subprocess
import sys
import uuid

from verify_runtime_matrix import OVERRIDES, PROFILE, check_file, sha

HOST_TOOLS = {'cc', 'gcc', 'c++', 'ar', 'ld', 'git', 'sh', 'bash',
              'dirname', 'tr', 'sha256sum', 'cut', 'mkdir', 'mktemp'}


def load_identity(identity):
    return json.loads(check_file(identity).read_text())


def tool_identity(tool, env):
    path = check_file(tool)
    if not os.access(path, os.X_OK):
        raise ValueError('runtime identity is not executable')
    version = subprocess.check_output([str(path), *tool['version_args']], env=env,
                                      text=True, timeout=20).strip()
    if version != tool['version']:
        raise ValueError('runtime identity version mismatch')
    return {'path': str(path), 'resolved_path': str(path.resolve()),
            'sha256': sha(path), 'version': version}


def selection_contract(selection):
    if selection['schema'] != 'kairotensor-project-runtime/1':
        raise ValueError('unknown selection schema')
    manifest = load_identity(selection['qualification_manifest'])
    receipt = load_identity(selection['qualification_receipt'])
    acceptance = load_identity(selection['acceptance'])
    check_file(selection['independent_review'])
    if (acceptance['result'] != 'G1 accepted'
            or acceptance['accepted_executable_source'] != manifest['source']['commit']
            or acceptance['independent_review'] != selection['independent_review']):
        raise ValueError('acceptance does not bind the qualification source/review')
    lanes = [lane for lane in manifest['lanes'] if lane['role'] == selection['role']]
    if len(lanes) != 1:
        raise ValueError('selection requires one qualified role')
    lane = lanes[0]
    if (receipt['status'] != 'Pass' or receipt['role'] != lane['role']
            or receipt['source'] != manifest['source']
            or receipt['manifest_sha256'] != hashlib.sha256(json.dumps(manifest, sort_keys=True).encode()).hexdigest()):
        raise ValueError('qualification receipt identity/status mismatch')
    for key in ('rustc', 'cargo', 'python'):
        expected = {field: lane[key][field] for field in ('path', 'sha256', 'version', 'version_args')}
        if selection['tools'][key] != expected:
            raise ValueError('selected tool identity differs from qualification')
        recorded = receipt['tools'][key]
        if any(recorded[field] != expected[field] for field in ('path', 'sha256', 'version')):
            raise ValueError('qualification tool identity mismatch')
    for item in [*manifest['native_files'], *lane['provenance'],
                 *(manifest[key] for key in ('wrapper', 'storage_guard', 'cargo_recipe'))]:
        check_file(item)
    if not manifest['native_files'] or not lane['provenance']:
        raise ValueError('missing native provenance')
    host_path = manifest['host_path']
    if not isinstance(host_path, str) or any(not part or not Path(part).is_absolute() for part in host_path.split(os.pathsep)):
        raise ValueError('qualification host PATH requires absolute entries')
    if set(selection['host_tools']) != HOST_TOOLS:
        raise ValueError('missing or unexpected native/helper identity')
    for name, identity in selection['host_tools'].items():
        path = check_file(identity)
        resolved = shutil.which(name, path=host_path)
        if not resolved or Path(resolved).resolve() != path.resolve():
            raise ValueError('native/helper resolution differs from qualified PATH: ' + name)
        if not os.access(path, os.X_OK):
            raise ValueError('native/helper identity is not executable: ' + name)
    # The actual cc/gcc used by the qualified bundled SQLite build is also bound
    # by G1's native-file inventory, not merely added to a new local selection.
    for name in ('cc', 'gcc'):
        if selection['host_tools'][name] not in manifest['native_files']:
            raise ValueError('native compiler differs from qualification: ' + name)
    return manifest


def runtime_environment(selection, manifest, aliases):
    env = dict(os.environ)
    forbidden = OVERRIDES | {'LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES',
                            'CARGO_BUILD_TARGET', 'CARGO_BUILD_RUSTDOC', 'CARGO_ALIAS_TEST',
                            'GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_CONFIG',
                            'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS', 'BASH_ENV', 'ENV',
                            'NIX_CFLAGS_COMPILE', 'NIX_LDFLAGS'}
    unexpected = [key for key in forbidden if env.get(key)]
    unexpected += [key for key in env if key.startswith(('CARGO_TARGET_', 'CARGO_PROFILE_', 'CARGO_ALIAS_', 'CARGO_BUILD_',
                                                                       'CC_', 'CXX_', 'AR_', 'CFLAGS_', 'CXXFLAGS_', 'HOST_CC', 'HOST_CXX', 'HOST_AR'))
                   and env[key] and env[key] != PROFILE.get(key)]
    if unexpected:
        raise ValueError('unexpected runtime overrides: ' + ', '.join(sorted(set(unexpected))))
    env.update(PROFILE)
    aliases.mkdir(mode=0o700)
    for alias, key in [('rustc', 'rustc'), ('cargo', 'cargo'), ('python3', 'python'), ('python', 'python')]:
        (aliases / alias).symlink_to(selection['tools'][key]['path'])
    for name, identity in selection['host_tools'].items():
        (aliases / name).symlink_to(identity['path'])
    env['PATH'] = str(aliases) + os.pathsep + manifest['host_path']
    env['RUSTC'] = selection['tools']['rustc']['path']
    return env


def python_probe(tools, env):
    expression = 'import os,sys,json;print(json.dumps({"realpath":os.path.realpath(sys.executable),"version":"Python "+sys.version.split()[0]}))'
    parent = json.loads(subprocess.check_output([tools['python']['path'], '-c', expression], env=env, text=True, timeout=20))
    child_code = 'import subprocess;print(subprocess.check_output(["python3","-c",' + repr(expression) + '],text=True),end="")'
    child = json.loads(subprocess.check_output([tools['python']['path'], '-c', child_code], env=env, text=True, timeout=20))
    expected = {'realpath': tools['python']['resolved_path'], 'version': tools['python']['version']}
    if parent != expected or child != expected:
        raise ValueError('selected parent/child Python mismatch')
    return {'parent': parent, 'child': child}


def source_record(root, env):
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args], env=env, text=True).strip()
    tracked = git('ls-files').splitlines()
    return {'root': str(root), 'commit': git('rev-parse', 'HEAD'),
            'dirty': bool(git('status', '--porcelain', '--untracked-files=normal')),
            'status': git('status', '--porcelain', '--untracked-files=normal'),
            'tracked': {name: sha(root / name) if (root / name).is_file() else None for name in tracked},
            'untracked': git('ls-files', '--others', '--exclude-standard').splitlines(),
            'lock_sha256': sha(root / 'Cargo.lock'), 'qualification': 'not assessed'}


def check_cargo_arguments(args, root, env):
    allowed = {'build', 'test', 'run', 'check', 'metadata', 'locate-project', '--version', '-V', 'version'}
    if (not args or args[0] not in allowed or any(arg.startswith('+')
                       or (arg.startswith('-') and not arg.startswith('--') and arg not in {'-V', '-q', '-v', '-vv'})
                       or arg.split('=')[0] in
                       {'--config', '--target-dir', '--manifest-path', '--profile', '--release', '-r', '--target', '--lockfile-path'} for arg in args)):
        raise ValueError('Cargo requires ordinary commands without runtime/output/profile overrides')
    # Cargo reads every ancestor config and Cargo home; refuse hidden selectors.
    cargo_home = Path(env.get('CARGO_HOME', str(Path.home() / '.cargo'))).absolute()
    directories = [root / '.cargo', *(parent / '.cargo' for parent in root.parents), cargo_home]
    if any((directory / name).exists() for directory in directories for name in ('config', 'config.toml')):
        raise ValueError('unbound Cargo config; review and bind it before use')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    state_default = Path(os.environ.get('XDG_STATE_HOME', str(Path.home() / '.local/state'))) / 'kairotensor/runtime'
    parser.add_argument('--selection', type=Path, default=state_default / 'accepted-selection.json')
    parser.add_argument('--state-dir', type=Path, default=state_default)
    parser.add_argument('--budget-gib', type=float)
    parser.add_argument('command', choices=['identity', 'python', 'cargo'])
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    record = None
    receipt_path = None
    try:
        if args.command == 'cargo' and (args.budget_gib is None or not math.isfinite(args.budget_gib) or args.budget_gib <= 0):
            raise ValueError('Cargo requires a positive finite --budget-gib')
        selection_path = args.selection.absolute()
        selection = json.loads(selection_path.read_text())
        manifest = selection_contract(selection)
        invocation = args.state_dir.absolute() / 'invocations' / (datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ-') + uuid.uuid4().hex)
        invocation.mkdir(parents=True, mode=0o700)
        env = runtime_environment(selection, manifest, invocation / 'bin')
        tools = {key: tool_identity(value, env) for key, value in selection['tools'].items()}
        probe = python_probe(tools, env)
        root = Path(subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], env=env, text=True).strip()).resolve()
        record = {'schema': 'kairotensor-project-invocation/1', 'selection': {'path': str(selection_path), 'sha256': sha(selection_path)},
                  'qualification_source': manifest['source']['commit'], 'source': source_record(root, env), 'host_tools': selection['host_tools'],
                  'tools': tools, 'python_probe': probe, 'profile': PROFILE, 'wrapper': manifest['wrapper'],
                  'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'exit': None}
        receipt_path = invocation / 'receipt.json'
        if args.command == 'identity':
            if args.args:
                raise ValueError('identity takes no arguments')
            command = []
        elif args.command == 'python':
            command = [tools['python']['path'], *args.args]
        else:
            check_cargo_arguments(args.args, root, env)
            command = [manifest['wrapper']['path'], '--budget-gib', format(args.budget_gib, 'g'), *args.args]
        record['command'] = command
        # Only command arguments and selected identities are recorded, never the environment.
        # Callers must keep secrets out of command arguments as with ordinary shell history.
        receipt_path.write_text(json.dumps(record, indent=2) + '\n')
        if command:
            result = subprocess.run(command, cwd=root, env=env)
            code = result.returncode
        else:
            print(json.dumps({'tools': tools, 'python_probe': probe, 'receipt': str(receipt_path)}, indent=2))
            code = 0
        for value in [*tools.values(), *selection['host_tools'].values()]:
            check_file(value)
        record['exit'] = code
        record['finished_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        receipt_path.write_text(json.dumps(record, indent=2) + '\n')
        return code
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        if record is not None and receipt_path is not None:
            record.update(exit=2, error=str(error))
            receipt_path.write_text(json.dumps(record, indent=2) + '\n')
        print(f'Project runtime selection refused: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
