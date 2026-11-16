"""Local, single-shot correction calibration. CLI: freeze/fixture/audit only.

LIVE needs an explicit trusted-caller LiveApproval; JSON never grants authority.
This is diagnostic accounting, not Store timed admission or a hard token budget.
"""
import argparse
import ctypes
import functools
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import selectors
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
MODEL = 'ornith-1.5:9b'
MODEL_DIGEST = 'e5df7dcdd8a263994df62d610317e07be0d6af23f96fcbd8543273058fce575e'
MANIFEST_SHA = '4e3abcdcb78587e3709cc65607f590347deb217b5e45f48113734915548d0ef8'
IMAGE = 'docker.io/ollama/ollama@sha256:85b3c895222b1ddc9d63c40b20fc74f37a37593dcbbdfbf28910a925e43ead00'
IMAGE_ID = 'aa25f838bb0303774bc7540716dadd62605fd0724febfccfc511e67ef12274a5'
MODEL_ROOT = Path('/home/neo/Projects/dev-storage/persistent/local-ai/ollama-candidate-0.40.16-20261111/models')
MODEL_MANIFEST = MODEL_ROOT / 'manifests/registry.ollama.ai/library/ornith-1.5/9b'
SERVER = 'http://127.0.0.1:11445'
SOURCE_FILES = ('examples/controlled_correction_pilot.rs', 'examples/controlled_correction_pilot/adapter.rs',
                'examples/controlled_correction_pilot/tests.rs', 'scripts/run_controlled_correction_pilot.py',
                'scripts/test_controlled_correction_pilot.py', 'docs/controlled-correction-pilot.md',
                'Cargo.lock', 'Cargo.toml')
OPTIONS = dict(seed=0, temperature=0, top_k=1, top_p=1, num_ctx=2048, num_predict=96, num_thread=2, num_gpu=0)
ENVELOPE = dict(model=MODEL, runner='ggml', stream=False, think=False, truncate=False,
                shift=False, keep_alive='10m', options=OPTIONS)
MAX_BYTES = 1024 * 1024
SCENARIOS = ('report_correction', 'unrelated_policy_change', 'withdrawal', 'alternative_support')


def clock():
    return time.clock_gettime(time.CLOCK_BOOTTIME)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON key')
        result[key] = value
    return result


def parse(raw):
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))


def final_receipt(response):
    message = response.get('message')
    return (response.get('model') == MODEL and response.get('done') is True and
            response.get('done_reason') in ('stop', 'length') and
            type(message) is dict and message.get('role') == 'assistant' and
            type(message.get('content')) is str and not message.get('thinking') and
            not message.get('tool_calls'))


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def sha(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def write_new(path, value, raw=False):
    data = value if raw else (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()
    with Path(path).open('xb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    fd = os.open(Path(path).parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Ledger:
    """Single in-process execution, no resume. A claim remains held on ambiguity."""
    def __init__(self, out, *, deadline, clock=clock):
        self.out, self.deadline, self.clock = Path(out), deadline, clock
        self.claims = 0
        self.pending = None
        self.blocked = False

    def claim(self, request):
        require(not self.blocked and self.pending is None, 'prior claim unresolved or run stopped')
        require(self.claims < 13, 'thirteenth call is the last permitted call')
        if self.clock() >= self.deadline - 60:
            raise TimeoutError('original execution work cutoff reached')
        ordinal = self.claims + 1
        # Persistence failure itself stops this process; it never authorizes transport.
        self.blocked = True
        write_new(self.out / f'claim-{ordinal:02}.json',
                  dict(ordinal=ordinal, request_utf8=request, request_sha256=digest(request.encode()),
                       observed_tick=self.clock(), quote_units=2144, quote_is_proven_bound=False))
        self.claims, self.pending, self.blocked = ordinal, ordinal, False
        return ordinal

    def receive(self, ordinal, raw):
        require(ordinal == self.pending and not self.blocked, 'no matching live claim')
        self.blocked = True
        tick = self.clock()
        raw_path = self.out / f'raw-{ordinal:02}.bin'
        if raw_path.exists():
            require(raw_path.is_file() and raw_path.read_bytes() == raw, 'streamed receipt differs from delivered bytes')
        else:
            write_new(raw_path, raw, raw=True)
        require(len(raw) <= MAX_BYTES, 'oversized raw receipt')
        response = parse(raw)
        require(type(response) is dict, 'response must be object')
        input_count, output_count = response.get('prompt_eval_count'), response.get('eval_count')
        require(type(input_count) is int and input_count >= 0 and
                type(output_count) is int and output_count >= 0, 'missing or malformed measured usage')
        message = response.get('message')
        final = final_receipt(response)
        # Persist measured excess too. Known bad output is not unknown cost or zero cost.
        usage = dict(ordinal=ordinal, input_tokens=input_count, output_tokens=output_count,
                     raw_sha256=digest(raw), received_tick=tick, late=tick >= self.deadline - 60,
                     within_envelope=0 < input_count <= 2048 and 0 < output_count <= 96,
                     reconciled=bool(final), transport_complete=True)
        write_new(self.out / f'usage-{ordinal:02}.json', usage)
        require(usage['within_envelope'], 'observed usage exceeds configured envelope; stopped without clamping')
        require(final, 'incomplete, wrong-model or unexpected response; liability remains unresolved')
        require(not usage['late'], 'late receipt retained; no further generation')
        self.pending, self.blocked = None, False
        return dict(ok=True, ordinal=ordinal, text=message['content'], input_tokens=input_count,
                    output_tokens=output_count)

    def observe_uncertain(self, ordinal):
        """Preserve readable usage on a failed transport without releasing its hold."""
        require(ordinal == self.pending, 'no matching uncertain claim')
        self.blocked = True
        path = self.out/f'raw-{ordinal:02}.bin'
        if not path.is_file():return
        raw = path.read_bytes()
        try:
            response = parse(raw)
            i, o = response.get('prompt_eval_count'), response.get('eval_count')
        except (ValueError, UnicodeError, AttributeError):
            return
        if type(i) is not int or i < 0 or type(o) is not int or o < 0:return
        write_new(self.out/f'usage-{ordinal:02}.json', dict(ordinal=ordinal,
            input_tokens=i, output_tokens=o, raw_sha256=digest(raw), received_tick=self.clock(),
            late=self.clock() >= self.deadline-60, within_envelope=0 < i <= 2048 and 0 < o <= 96,
            reconciled=False, transport_complete=False))


def audit(out):
    out = Path(out)
    claims = sorted(out.glob('claim-*.json'))
    known = 0
    unresolved = 0
    observed_excess = False
    for expected, path in enumerate(claims, 1):
        claim = parse(path.read_bytes())
        require(path.name == f'claim-{expected:02}.json' and claim['ordinal'] == expected,
                'noncontiguous claims')
        require(digest(claim['request_utf8'].encode()) == claim['request_sha256'], 'request digest mismatch')
        usage_path = out / f'usage-{expected:02}.json'
        if not usage_path.exists():
            unresolved += 1
            continue
        usage = parse(usage_path.read_bytes())
        raw = (out / f'raw-{expected:02}.bin').read_bytes()
        require(usage['ordinal'] == expected and usage['raw_sha256'] == digest(raw), 'receipt digest mismatch')
        response = parse(raw)
        i, o = usage['input_tokens'], usage['output_tokens']
        require(type(i) is int and i >= 0 and type(o) is int and o >= 0 and
                response.get('prompt_eval_count') == i and response.get('eval_count') == o,
                'usage mismatch')
        require(type(usage['transport_complete']) is bool and
                usage['reconciled'] == bool(final_receipt(response) and usage['transport_complete']) and
                usage['within_envelope'] == (0 < i <= 2048 and 0 < o <= 96), 'receipt classification mismatch')
        known += i + o
        if usage.get('reconciled') is not True:
            unresolved += 1
        observed_excess |= not (0 < i <= 2048 and 0 < o <= 96)
    require(len(claims) <= 13, 'claim ceiling exceeded')
    require({p.name for p in out.glob('usage-*.json')} <=
            {f'usage-{n:02}.json' for n in range(1, len(claims)+1)}, 'orphan usage receipt')
    return dict(claims=len(claims), known_tokens=known, unresolved_claims=unresolved,
                unresolved_quote_units=2144 * unresolved, observed_excess=observed_excess,
                hard_token_liability_bound=None,
                accounting_scope='provider-reported local token units; no proven unknown-liability bound')


def schedule_outcomes(out, claims):
    store = Path(out) / 'store'
    manifest = store / 'manifest.json'
    slots = parse(manifest.read_bytes()).get('planned_slots', []) if manifest.exists() else []
    completed = set()
    for path in sorted(store.glob('event-*.json')):
        event = parse(path.read_bytes())
        if event.get('kind') == 'pair_complete':
            completed.add(event['result']['scenario'])
    return dict(unexecuted_slots=slots[claims:], scenario_outcomes={name:
                'complete' if name in completed else
                'incomplete' if any(slot.startswith(name + '/') for slot in slots[:claims]) else
                'unexecuted' for name in SCENARIOS})


@dataclass(frozen=True)
class LiveApproval:
    """Trusted caller attestation, not an unforgeable permission capability."""
    source_commit: str
    binary_sha256: str
    config_sha256: str
    review_sha256: str


def run(args, timeout=20):
    p = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, timeout=timeout)
    if p.returncode:
        raise ValueError(f'command failed ({p.returncode}): {args[0]} {args[1] if len(args)>1 else ""}: {p.stderr[-2000:]}')
    return p.stdout.strip()


def source_identity():
    require(not run(['git', 'status', '--porcelain', '--untracked-files=normal']), 'freeze requires clean source')
    return dict(commit=run(['git', 'rev-parse', 'HEAD']),
                files={name: sha(ROOT / name) for name in SOURCE_FILES})


def assets():
    require(sha(MODEL_MANIFEST) == MANIFEST_SHA, 'local model manifest changed')
    manifest = parse(MODEL_MANIFEST.read_bytes())
    files = []
    for item in [manifest['config'], *manifest['layers']]:
        algorithm, value = item['digest'].split(':')
        require(algorithm == 'sha256' and re.fullmatch('[0-9a-f]{64}', value), 'bad model digest')
        path = MODEL_ROOT / 'blobs' / ('sha256-' + value)
        require(path.is_file() and not path.is_symlink() and path.stat().st_size == item['size'], 'missing or changed model file')
        require(sha(path) == value, 'model content digest mismatch')
        files.append(dict(path=str(path), bytes=item['size'], sha256=value, mtime_ns=path.stat().st_mtime_ns))
    return files


def freeze(binary):
    binary = Path(binary).resolve()
    require(binary.is_file() and os.access(binary, os.X_OK), 'compiled executable required')
    podman = Path(shutil.which('podman') or '').resolve()
    image = parse(run([str(podman), 'image', 'inspect', IMAGE_ID]))[0]
    require(image['Id'].removeprefix('sha256:') == IMAGE_ID, 'image identity changed')
    return dict(schema='controlled-correction-execution/1', execution_authorized=False,
                source=source_identity(), binary=dict(path=str(binary), sha256=sha(binary)),
                python=dict(path=str(Path(sys.executable).resolve()), sha256=sha(sys.executable)),
                podman=dict(path=str(podman), sha256=sha(podman)), image=IMAGE, image_id=IMAGE_ID,
                model=MODEL, model_digest=MODEL_DIGEST, model_manifest_sha256=MANIFEST_SHA,
                model_files=assets(), envelope=ENVELOPE, max_calls=13, wall_seconds=1800,
                request_timeout_seconds=120, configured_input_limit=2048, requested_output_limit=96,
                aggregate_quote_units=27872, hard_token_budget=False,
                platform=platform.platform(), cpu_affinity=sorted(os.sched_getaffinity(0)),
                new_model_calls=0, new_training_jobs=0)


def verify_config(config, approval):
    require(config['schema'] == 'controlled-correction-execution/1' and config['execution_authorized'] is False,
            'configuration cannot grant execution')
    require(config['source'] == source_identity() and config['source']['commit'] == approval.source_commit,
            'source no longer matches reviewed candidate')
    require(config['binary']['sha256'] == approval.binary_sha256 and
            sha(config['binary']['path']) == approval.binary_sha256, 'binary identity mismatch')
    require(config['python']['sha256'] == sha(sys.executable), 'actual Python differs from freeze')
    require(config['podman']['sha256'] == sha(config['podman']['path']), 'Podman identity changed')
    require(config['image'] == IMAGE and config['image_id'] == IMAGE_ID and
            config['model'] == MODEL and config['model_digest'] == MODEL_DIGEST and
            config['model_manifest_sha256'] == MANIFEST_SHA and config['envelope'] == ENVELOPE,
            'runtime/request contract changed')
    require(config['max_calls'] == 13 and config['wall_seconds'] == 1800 and
            config['request_timeout_seconds'] == 120 and config['hard_token_budget'] is False,
            'execution bounds changed')
    require(config['model_files'] == assets(), 'model assets changed since reviewed freeze')


def validate_request(record, ordinal):
    require(set(record) == {'op', 'ordinal', 'model_digest', 'request_utf8'} and
            record['op'] == 'generate' and record['ordinal'] == ordinal and
            record['model_digest'] == MODEL_DIGEST, 'unexpected IPC claim')
    raw = record['request_utf8']
    require(type(raw) is str and len(raw.encode()) <= 16384, 'request too large')
    request = parse(raw)
    require(set(request) == set(ENVELOPE) | {'messages'} and
            json.dumps({k: v for k, v in request.items() if k != 'messages'}, sort_keys=True) ==
            json.dumps(ENVELOPE, sort_keys=True), 'request settings changed')
    messages = request['messages']
    require(type(messages) is list and len(messages) == 2 and
            all(type(m) is dict and set(m) == {'role', 'content'} and type(m['content']) is str for m in messages) and
            [m['role'] for m in messages] == ['system', 'user'], 'unexpected messages')
    source = (ROOT / 'examples/controlled_correction_pilot.rs').read_text()
    prompts = [re.search(r'const ' + name + r':\s*&str\s*=\s*r#"(.*?)"#;', source, re.S).group(1)
               for name in ('REPORT_PROMPT', 'PLAN_PROMPT')]
    require(messages[0]['content'] in prompts, 'unfrozen prompt')
    data = parse(messages[1]['content'])
    require(type(data) is dict and set(data) in ({'ci_report'}, {'requirement', 'test_certificate', 'report_version'}),
            'unexpected task payload')
    return raw


class HttpFailure(ValueError):
    pass


def http(path, body=None, timeout=3, raw_path=None):
    require(path in ('/api/version', '/api/tags', '/api/ps', '/api/show', '/api/chat'), 'endpoint forbidden')
    require(timeout > 0 and math.isfinite(timeout), 'invalid request duration')
    deadline = clock() + timeout
    payload = json.dumps(dict(url=SERVER+path, body=None if body is None else body.decode(), timeout=timeout)).encode()
    worker_code = ('import sys;sys.path.insert(0,sys.argv[1]);'
                   'import run_controlled_correction_pilot as p;p.http_worker()')
    process = subprocess.Popen([sys.executable, '-c', worker_code, str(ROOT/'scripts')],
              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
              start_new_session=True, preexec_fn=functools.partial(child_setup, os.getpid()))
    sink = None
    data = bytearray()
    header = bytearray()
    status = None
    try:
        if raw_path is not None:
            sink = Path(raw_path).open('xb')
            sink.flush();os.fsync(sink.fileno())
            fd = os.open(Path(raw_path).parent, os.O_RDONLY | os.O_DIRECTORY)
            try:os.fsync(fd)
            finally:os.close(fd)
        process.stdin.write(payload);process.stdin.close()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            eof = False
            while not eof:
                remaining = deadline-clock()
                if remaining <= 0:
                    raise TimeoutError('absolute HTTP request deadline exceeded; partial receipt retained')
                if not selector.select(min(0.05, remaining)):
                    continue
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    eof = True
                    break
                if status is None:
                    header.extend(chunk)
                    require(len(header) <= MAX_BYTES, 'invalid HTTP worker header')
                    if b'\n' not in header:
                        continue
                    line, chunk = bytes(header).split(b'\n', 1)
                    status = parse(line)['status']
                if chunk:
                    data.extend(chunk)
                    if sink is not None:
                        sink.write(chunk);sink.flush();os.fsync(sink.fileno())
                    require(len(data) <= MAX_BYTES, 'response exceeds artifact limit')
            code = process.wait(timeout=max(0.001, deadline-clock()))
            if code != 0 or status != 200:
                raise HttpFailure(f'HTTP transport/status failure: exit={code}, status={status}; partial receipt retained')
            require(clock() <= deadline, 'HTTP request exceeded its original deadline')
            return bytes(data)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        if sink is not None:sink.close()
        if not process.stdin.closed:process.stdin.close()
        process.stdout.close();process.stderr.close()


def http_worker():
    """Private subprocess helper; parent enforces elapsed time and saves every chunk."""
    from urllib.parse import urlsplit
    value = parse(sys.stdin.buffer.read(32768))
    url = urlsplit(value['url'])
    require(url.scheme == 'http' and url.hostname == '127.0.0.1' and not url.username and not url.password,
            'worker only permits unauthenticated loopback HTTP')
    body = None if value['body'] is None else value['body'].encode()
    request = urllib.request.Request(value['url'], data=body,
                headers={'Content-Type': 'application/json'}, method='POST' if body is not None else 'GET')
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            raise ValueError('redirect forbidden')
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        response = opener.open(request, timeout=value['timeout'])
    except urllib.error.HTTPError as error:
        response = error
    with response:
        sys.stdout.buffer.write((json.dumps({'status':response.status})+'\n').encode());sys.stdout.buffer.flush()
        count = 0
        while True:
            chunk = response.read1(1024)
            if not chunk:
                require(response.length in (None, 0), 'HTTP body ended before Content-Length')
                break
            count += len(chunk)
            sys.stdout.buffer.write(chunk);sys.stdout.buffer.flush()
            require(count <= MAX_BYTES, 'oversized HTTP body')


def child_setup(expected_parent):
    # Owned Rust child dies if this Python controller disappears. No sibling kill.
    parent = expected_parent
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGKILL, 0, 0, 0) != 0 or os.getppid() != parent:
        os._exit(127)


def read_record(process, deadline):
    data = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        while clock() < deadline - 60:
            if not selector.select(min(0.25, max(0, deadline - 60 - clock()))):
                continue
            chunk = os.read(process.stdout.fileno(), 1)
            if not chunk:
                if data:
                    raise ValueError('partial IPC at EOF')
                return None
            data.extend(chunk)
            require(len(data) <= MAX_BYTES, 'oversized IPC')
            if chunk == b'\n':
                return parse(data)
    raise TimeoutError('original work cutoff reached during IPC')


def validate_cpu_observation(value):
    models = value.get('models')
    require(type(models) is list and len(models) == 1, 'expected one owned loaded model')
    model = models[0]
    require(model.get('name') == MODEL and model.get('digest') == MODEL_DIGEST and
            type(model.get('size_vram')) is int and model['size_vram'] == 0 and
            type(model.get('context_length')) is int and 0 < model['context_length'] <= 2048,
            'loaded runtime is not the frozen CPU/context configuration')


def container_command(podman, name, remaining):
    # --timeout is enforced by conmon outside this controller. No shared runtime changes.
    return [podman, 'run', '--detach', '--name', name, '--pull=never', '--timeout', str(remaining),
            '--cpus', '2', '--memory', '12g', '--memory-swap', '12g', '--restart=no',
            '--pids-limit', '128', '--read-only', '--cap-drop=ALL', '--security-opt=no-new-privileges',
            '--network=pasta:--no-map-gw', '--publish=127.0.0.1:11445:11434',
            '--tmpfs=/root/.ollama:rw,size=16m,mode=700',
            '--tmpfs=/tmp:rw,size=256m,mode=1777',
            '--mount', f'type=bind,src={MODEL_ROOT},dst=/root/.ollama/models,ro',
            '--env=OLLAMA_HOST=0.0.0.0:11434', '--env=OLLAMA_NO_CLOUD=1',
            '--env=OLLAMA_MAX_LOADED_MODELS=1', '--env=OLLAMA_NUM_PARALLEL=1',
            IMAGE_ID, 'serve']


def process_start(pid):
    try:
        return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
    except (OSError, IndexError):
        return None


def owned_start(command, out, deadline):
    """Bound the creation CLI itself; a lost reply never authorizes another create."""
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              start_new_session=True, preexec_fn=functools.partial(child_setup, os.getpid()))
    stdout, stderr = b'', b''
    try:
        write_new(out/'startup-process.json', dict(pid=process.pid, start=process_start(process.pid)))
        stdout, stderr = process.communicate(timeout=max(.01, min(30, deadline-clock())))
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate()
        process.wait()
        write_new(out/'startup-finished.json', dict(pid=process.pid, exit=process.returncode,
                  stdout=stdout.decode(errors='replace'), stderr=stderr.decode(errors='replace'), tick=clock()))
    require(process.returncode == 0, 'owned runtime creation failed; inspect startup receipt')
    return stdout.decode().strip()


def drive(binary, out, ledger, deadline, transport, after_reply=None):
    """Real Rust IPC and accounting; tests replace only the external model transport."""
    process = None
    try:
        with (out / 'rust-stderr.log').open('xb') as stderr:
            process = subprocess.Popen([binary, '--bridge', str(out / 'store')],
                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
                       start_new_session=True, preexec_fn=functools.partial(child_setup, os.getpid()))
            while True:
                record = read_record(process, deadline)
                if record is None:
                    break
                request = validate_request(record, ledger.claims + 1)
                ordinal = ledger.claim(request)
                try:
                    raw = transport(request.encode(), min(120, max(1, deadline - 60 - clock())))
                except BaseException:
                    ledger.observe_uncertain(ordinal)
                    raise
                reply = ledger.receive(ordinal, raw)
                if after_reply is not None:
                    after_reply(ordinal)
                process.stdin.write((json.dumps(reply) + '\n').encode())
                process.stdin.flush()
            code = process.wait(timeout=max(1, min(5, deadline - clock())))
            result = parse((out / 'store/result.json').read_bytes())
            require(code == 0 and result.get('complete') is True and result.get('dispatches') == ledger.claims,
                    'Rust Store pilot incomplete')
            return result
    finally:
        if process is not None:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            process.stdin.close()
            process.stdout.close()


def run_live(config_path, out, *, approval=None):
    # Must precede path reads, output creation, asset inspection and runtime commands.
    if type(approval) is not LiveApproval:
        raise PermissionError('LIVE requires a separate trusted-caller LiveApproval after independent review')
    started = clock()
    deadline = started + 1800
    raw_config = Path(config_path).read_bytes()
    require(digest(raw_config) == approval.config_sha256 and
            re.fullmatch('[0-9a-f]{64}', approval.review_sha256), 'approval does not bind configuration/review')
    config = parse(raw_config)
    verify_config(config, approval)
    require(clock() < deadline - 60, 'preflight exhausted execution window')
    out = Path(out)
    out.mkdir(mode=0o700)
    write_new(out / 'execution.json', dict(config=config, approval=approval.__dict__,
              started_tick=started, deadline_tick=deadline, clock='CLOCK_BOOTTIME', local_provider_spend_usd=0))
    ledger = Ledger(out, deadline=deadline)
    podman = config['podman']['path']
    name = 'kt-correction-' + uuid.uuid4().hex
    watchdog = None
    container = None
    error = None
    complete = False
    # A unique name plus --pull=never and kernel/container timeout contains only owned work.
    try:
        remaining = math.floor(deadline - clock())
        require(remaining > 60, 'no runtime startup window')
        command = container_command(podman, name, remaining)
        write_new(out / 'runtime-launch.json', dict(name=name, command=command, remaining_seconds=remaining))
        watchdog = subprocess.Popen([sys.executable, __file__, '_watchdog', '--name', name,
                   '--deadline', str(deadline), '--out', str(out), '--podman', podman],
                   stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   start_new_session=True)
        container = owned_start(command, out, deadline)
        require(re.fullmatch('[0-9a-f]{64}', container), 'unexpected container ID')
        write_new(out / 'runtime-created.json', dict(id=container, name=name))
        ready = False
        while clock() < min(started + 120, deadline - 60):
            try:
                version = parse(http('/api/version'))
                require(version.get('version') == '0.40.16', 'wrong runtime version')
                tags = parse(http('/api/tags'))
                selected = [m for m in tags.get('models', []) if m.get('name') == MODEL]
                require(len(selected) == 1 and selected[0].get('digest') == MODEL_DIGEST, 'wrong cached model variant')
                show = parse(http('/api/show', json.dumps({'model': MODEL, 'verbose': False}).encode()))
                initial_ps = parse(http('/api/ps'))
                require(initial_ps.get('models') == [], 'owned runtime unexpectedly had a loaded model')
                require('thinking' in show.get('capabilities', []) and
                        'enable_thinking is false' in show.get('template', ''), 'cached template lacks verified no-thinking path')
                write_new(out / 'runtime-ready.json', dict(version=version, selected=selected[0], show=show, ps=initial_ps,
                          inspect=parse(run([podman, 'inspect', container]))[0]))
                ready = True
                break
            except (OSError, urllib.error.URLError, HttpFailure):
                time.sleep(0.1)
        require(ready, 'owned runtime failed readiness')
        def transport(request, timeout):
            tags = parse(http('/api/tags'))
            require(any(m.get('name') == MODEL and m.get('digest') == MODEL_DIGEST for m in tags.get('models', [])),
                    'model alias changed; claimed request retained without retry')
            return http('/api/chat', request, timeout=timeout, raw_path=out/f'raw-{ledger.claims:02}.bin')
        def observe_cpu(ordinal):
            observation = parse(http('/api/ps'))
            write_new(out / f'runtime-ps-{ordinal:02}.json', observation)
            validate_cpu_observation(observation)
        drive(config['binary']['path'], out, ledger, deadline, transport, observe_cpu)
        complete = True
    except BaseException as exc:
        error = f'{type(exc).__name__}: {exc}'
    finally:
        # Even a lost create reply must reconcile the unique owned name; never touch the shared service.
        cleanup = {}
        try:
            inspected = parse(run([podman, 'inspect', name]))[0]
            require(inspected['Name'].lstrip('/') == name and inspected['Image'].removeprefix('sha256:') == IMAGE_ID,
                    'owned container identity mismatch at cleanup')
            owned_id = inspected['Id']
            if inspected['State'].get('Running'):
                run([podman, 'kill', '--signal=KILL', owned_id])
            logs = subprocess.run([podman, 'logs', owned_id], capture_output=True, timeout=10)
            write_new(out / 'runtime-log.txt', logs.stdout + logs.stderr, raw=True)
            cleanup = parse(run([podman, 'inspect', owned_id]))[0]
            require(cleanup['State'].get('Running') is False, 'owned runtime remains active')
            write_new(out / 'runtime-final.json', cleanup)
            # Retain stopped container and its writable tmpfs lifecycle evidence; owner can retire later.
        except Exception as exc:
            complete = False
            cleanup = dict(error=f'{type(exc).__name__}: {exc}')
            write_new(out / 'runtime-cleanup-error.json', cleanup)
        if watchdog is not None:
            try:
                # EOF always means parent loss. Only this explicit completion marker disables the watcher.
                if not cleanup.get('error'):
                    watchdog.stdin.write(b'done\n')
                    watchdog.stdin.flush()
                watchdog.stdin.close()
                watchdog.wait(timeout=15)
                require(watchdog.returncode == 0, 'watchdog failed')
            except Exception as exc:
                complete = False
                error = error or f'watchdog completion: {exc}'
        finished = clock()
        try:
            accounting = audit(out)
            schedule = schedule_outcomes(out, accounting['claims'])
        except Exception as exc:
            complete = False
            accounting = dict(error=f'{type(exc).__name__}: {exc}', authoritative_totals=False)
            schedule = dict(error='accounting audit failed; inspect retained schedule and events')
        complete = complete and not error and not accounting.get('unresolved_claims') and finished <= deadline
        report = dict(complete=bool(complete), error=error, started_tick=started, finished_tick=finished,
                      elapsed_seconds=finished-started, deadline_overrun=finished>deadline, accounting=accounting,
                      model_provider_spend_usd=0,
                      local_compute_cost='unpriced', shared_runtime_modified=False,
                      configured_token_envelope_is_hard_budget=False, schedule=schedule)
        write_new(out / 'supervisor.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('freeze');p.add_argument('--binary', required=True);p.add_argument('--out', required=True)
    p = sub.add_parser('fixture');p.add_argument('--binary', required=True);p.add_argument('--out', required=True)
    p = sub.add_parser('audit');p.add_argument('out')
    p = sub.add_parser('_watchdog', help=argparse.SUPPRESS)
    p.add_argument('--name', required=True);p.add_argument('--deadline', type=float, required=True)
    p.add_argument('--out', required=True);p.add_argument('--podman', required=True)
    args = parser.parse_args()
    if args.command == '_watchdog':
        return watchdog_main(args)
    if args.command == 'freeze':
        result = freeze(args.binary);write_new(Path(args.out), result);print(json.dumps(result, indent=2));return 0
    if args.command == 'fixture':
        return subprocess.run([args.binary, '--fixture', args.out], timeout=120).returncode
    print(json.dumps(audit(args.out), indent=2));return 0


def watch_pipe(fd, deadline, stop, clock=clock):
    with selectors.DefaultSelector() as selector:
        selector.register(fd, selectors.EVENT_READ)
        while clock() < deadline:
            if selector.select(min(0.1, max(0, deadline-clock()))):
                message = os.read(fd, 5)
                if message == b'done\n':
                    return 'completed'
                if not message:
                    stop()
                    return 'parent_lost'
                raise ValueError('invalid watchdog completion message')
        stop()
        return 'deadline'


def watchdog_main(args):
    require(re.fullmatch('kt-correction-[0-9a-f]{32}', args.name), 'not an owned pilot runtime name')
    require(math.isfinite(args.deadline), 'invalid absolute deadline')
    actions = []
    cleanup_verified = False
    def stop():
        nonlocal cleanup_verified
        startup = Path(args.out)/'startup-process.json'
        if startup.exists():
            record = parse(startup.read_bytes())
            pid = record['pid']
            if record.get('start') is not None and process_start(pid) == record['start']:
                try:
                    require(os.getpgid(pid) == pid, 'creation process is not the owned group leader')
                    os.killpg(pid, signal.SIGKILL)
                    actions.append(dict(startup_pid=pid, startup_kill='SIGKILL'))
                except ProcessLookupError:
                    pass
        # Absence during creation is unresolved. Keep reconciling through the original deadline.
        while True:
            p = subprocess.run([args.podman, 'inspect', args.name], capture_output=True, timeout=3)
            if p.returncode == 0:
                value = parse(p.stdout)[0]
                require(value['Name'].lstrip('/') == args.name and
                        value['Image'].removeprefix('sha256:') == IMAGE_ID, 'wrong owned runtime identity')
                if value['State'].get('Running'):
                    killed = subprocess.run([args.podman, 'kill', '--signal=KILL', value['Id']],
                                            capture_output=True, timeout=3)
                    actions.append(dict(id=value['Id'], kill_exit=killed.returncode))
                    # Observe stopped state even if the kill itself crossed the deadline.
                    final = subprocess.run([args.podman, 'inspect', value['Id']], capture_output=True, timeout=3)
                    if final.returncode == 0:
                        stopped = parse(final.stdout)[0]
                        cleanup_verified = (stopped['Id'] == value['Id'] and
                                            stopped['State'].get('Running') is False)
                    break
                else:
                    actions.append(dict(id=value['Id'], already_stopped=True))
                    cleanup_verified = True
                    break
            if clock() >= args.deadline:
                actions.append(dict(unresolved='owned runtime never observed; absence during creation is not stopped proof'))
                break
            time.sleep(min(0.1, max(0, args.deadline-clock())))
    error = None
    try:
        reason = watch_pipe(sys.stdin.fileno(), args.deadline, stop)
        if reason == 'completed':cleanup_verified = True
    except BaseException as exc:
        reason, error = 'error', f'{type(exc).__name__}: {exc}'
    write_new(Path(args.out) / 'watchdog.json', dict(reason=reason, actions=actions,
              deadline=args.deadline, observed_tick=clock(), cleanup_verified=cleanup_verified, error=error))
    return 0 if cleanup_verified and error is None else 2


if __name__ == '__main__':
    raise SystemExit(main())
