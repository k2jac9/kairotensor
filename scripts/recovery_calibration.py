"""Bounded calibration lab, NOT a new heldout evaluation or production controller.

Default fixture uses scripted UTF-8 byte tokens and makes zero model calls.
--live explicitly loads the pinned CPU model, once per dtype, on this machine.
No warmups, transport retries, automatic resumes, corpus loading or hidden calls.
"""
import argparse
import collections
import gc
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import resource
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
PROTOCOL_PATH = Path(__file__).with_name('recovery_calibration_protocol.json')
PROMPT_PATH = ROOT / 'examples/recovery_policy_eval/prompt-v2.json'
DTYPES = ('bfloat16', 'float32')
FORMATS = ('free', 'candidate_trie')
VALIDATOR = 'independent-report-verdict/1'
PROTOCOL_SHA256 = '099aeef00ade283e2001cfa443600686499786b797291d316480d2dcac8116d1'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f'duplicate JSON key: {key}')
        result[key] = value
    return result


def parse_json(text):
    return json.loads(text, object_pairs_hook=unique_object,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f'nonfinite JSON: {value}')))


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False)


def digest(value):
    return hashlib.sha256(value if isinstance(value, bytes) else value.encode('utf-8')).hexdigest()


def admitted_report(reports, admitted):
    require(admitted is True, 'evidence is not admitted')
    require(type(reports) is list and len(reports) == 1, 'exactly one complete report is required')
    report = reports[0]
    require(type(report) is dict and set(report) == {'release', 'passed', 'total'}, 'invalid report fields')
    require(type(report['release']) is str and bool(report['release']), 'release must be nonempty')
    require(type(report['passed']) is int and type(report['total']) is int,
            'counts must be integers, not booleans')
    require(0 <= report['passed'] <= report['total'] <= 2**64 - 1 and report['total'] > 0, 'invalid counts')
    return report


def structured_candidates(reports, admitted):
    """No reference label or status calculation enters this construction.

    Both pass and fail remain available, in fixed pass/fail order. Release and
    count extraction are supplied by code, which is a deliberate confound.
    """
    report = admitted_report(reports, admitted)
    return [json.dumps(dict(release=report['release'], tests=f"{report['passed']}/{report['total']}", status=status),
                       ensure_ascii=False, separators=(',', ':')) for status in ('pass', 'fail')]


def strict_validate(text, reference):
    """Mirror frozen strict string-field equality, without stripping fences."""
    try:
        answer = parse_json(text)
    except (ValueError, TypeError):
        return False
    return (type(answer) is dict and set(answer) == {'release', 'tests', 'status'}
            and all(type(value) is str for value in answer.values()) and answer == reference)


def load_protocol():
    require(digest(PROTOCOL_PATH.read_bytes()) == PROTOCOL_SHA256, 'frozen calibration protocol digest mismatch')
    protocol = parse_json(PROTOCOL_PATH.read_text())
    require(protocol['schema_version'] == 1 and protocol['protocol_id'] == 'controlled-recovery-calibration/1', 'unsupported protocol')
    require(protocol['validator_version'] == VALIDATOR and protocol['max_calls'] == 8
            and protocol['output_cap'] == 96 and protocol['threads'] == 2
            and protocol['walltime_seconds'] == 1500 and protocol['budget_tokens_per_call'] == 4096,
            'frozen limits changed')
    require({case['case_id'] for case in protocol['cases']} == {'cal-pass', 'cal-repair'}
            and len(protocol['cases']) == 2, 'only the two frozen calibration cases may execute')
    for case in protocol['cases']:
        require(case['split'] == 'calibration', 'heldout input is forbidden')
        report = admitted_report(case['reports'], case['admitted'])
        expected = dict(release=report['release'], tests=f"{report['passed']}/{report['total']}",
                        status='pass' if report['passed'] == report['total'] else 'fail')
        require(case['reference'] == expected, 'independent reference inconsistent with report')
    call_plan(protocol)
    prompt = parse_json(PROMPT_PATH.read_text())
    require(digest(PROMPT_PATH.read_bytes()) == protocol['prompt_sha256'], 'frozen prompt digest mismatch')
    require(prompt['prompt_version'] == protocol['prompt_version'] and prompt['validator_version'] == VALIDATOR,
            'prompt contract changed')
    return protocol


def call_plan(protocol):
    order = protocol['call_order']
    expected = {(case, dtype, mode) for case in ('cal-pass', 'cal-repair') for dtype in DTYPES for mode in FORMATS}
    require(type(order) is list and len(order) == 8, 'exactly eight planned calls required')
    require(all(set(cell) == {'case_id', 'dtype', 'format'} for cell in order), 'invalid call cell')
    require({(c['case_id'], c['dtype'], c['format']) for c in order} == expected, 'factorial cells missing or duplicated')
    # Two sequential model loads are intentional; there is never a second model
    # resident merely to balance ordering. This remains a cold/order confound.
    require([c['dtype'] for c in order] == ['bfloat16'] * 4 + ['float32'] * 4, 'unexpected model load order')
    return [dict(cell, ordinal=i + 1, dtype_position=i % 4 + 1, first_after_load=i % 4 == 0)
            for i, cell in enumerate(order)]


class CandidateTrie:
    def __init__(self, tokenizer, candidates, eos_ids, output_cap):
        require(len(candidates) == 2 and len(set(candidates)) == 2, 'exactly two distinct candidates required')
        require(bool(eos_ids) and all(type(t) is int and t >= 0 for t in eos_ids), 'valid EOS tokens required')
        self.eos_ids = tuple(sorted(set(eos_ids)))
        self.candidates = tuple(candidates)
        self.token_paths = []
        self.edges = collections.defaultdict(set)
        self.terminals = set()
        self.text_by_path = {}
        for text in candidates:
            tokens = tuple(tokenizer.encode(text, add_special_tokens=False))
            require(tokens and all(type(t) is int and t >= 0 for t in tokens), 'invalid candidate token IDs')
            require(not set(tokens).intersection(self.eos_ids), 'candidate contains EOS before completion')
            decoded = tokenizer.decode(list(tokens), skip_special_tokens=False, clean_up_tokenization_spaces=False)
            require(decoded == text, 'candidate tokenizer decode round-trip mismatch')
            require(len(tokens) + 1 <= output_cap, 'candidate plus EOS exceeds output cap')
            self.token_paths.append(tokens)
            for index, token in enumerate(tokens):
                self.edges[tokens[:index]].add(token)
            self.terminals.add(tokens)
            self.text_by_path[tokens] = text
        require(len(set(self.token_paths)) == 2, 'different candidates encoded identically')

    def allowed(self, generated):
        prefix = tuple(generated)
        if prefix in self.terminals:
            return list(self.eos_ids)
        require(prefix in self.edges, 'generation departed from frozen candidate trie')
        return sorted(self.edges[prefix])

    def callback(self, prompt_tokens):
        prompt = tuple(prompt_tokens)
        def allowed_tokens(batch_id, full_ids):
            require(batch_id == 0, 'only one generation sequence is permitted')
            full = tuple(full_ids.tolist() if hasattr(full_ids, 'tolist') else full_ids)
            require(full[:len(prompt)] == prompt, 'generation prompt token IDs changed')
            return self.allowed(full[len(prompt):])
        return allowed_tokens

    def verify(self, completion, text):
        require(len(completion) >= 2 and completion[-1] in self.eos_ids, 'structured response did not terminate with EOS')
        require(tuple(completion[:-1]) in self.terminals, 'structured response is not a complete candidate')
        require(self.text_by_path[tuple(completion[:-1])] == text, 'structured decoded text does not match its token path')


class ByteTokenizer:
    """Mechanical fixture tokenizer; its vocabulary is bytes, not Qwen tokens."""
    eos_token_id = 256
    def encode(self, text, add_special_tokens=False):
        require(not add_special_tokens, 'fixture encoder expects raw candidate text')
        return list(text.encode('utf-8'))
    def decode(self, tokens, skip_special_tokens=False, clean_up_tokenization_spaces=False):
        if skip_special_tokens:
            tokens = [token for token in tokens if token != self.eos_token_id]
        return bytes(tokens).decode('utf-8')
    def apply_chat_template(self, messages, tokenize=False, add_generation_prompt=True):
        rendered = ''.join(f"<|{m['role']}|>\n{m['content']}\n" for m in messages)
        if add_generation_prompt:
            rendered += '<|assistant|>\n'
        return self.encode(rendered) if tokenize else rendered


def read_text_if_available(path):
    try:
        return Path(path).read_text()
    except OSError:
        return None


def memory_state():
    meminfo = read_text_if_available('/proc/meminfo') or ''
    status = read_text_if_available('/proc/self/status') or ''
    def field(text, name):
        line = next((line for line in text.splitlines() if line.startswith(name + ':')), None)
        return None if line is None else line.split(':', 1)[1].strip()
    return {'available_memory': field(meminfo, 'MemAvailable'), 'process_rss': field(status, 'VmRSS'),
            'peak_rss_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
            'cgroup_memory_max': read_text_if_available('/sys/fs/cgroup/memory.max'),
            'cgroup_memory_current': read_text_if_available('/sys/fs/cgroup/memory.current')}


def machine_metadata():
    cpuinfo = read_text_if_available('/proc/cpuinfo') or ''
    model = next((line.split(':', 1)[1].strip() for line in cpuinfo.splitlines() if line.startswith('model name')), None)
    flags = sorted(set(next((line.split(':', 1)[1].split() for line in cpuinfo.splitlines() if line.startswith('flags')), [])))
    affinity = sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None
    profile = {'cpu_model': model, 'flags': flags, 'affinity': affinity, 'logical_cpus': os.cpu_count()}
    return dict(profile, cpu_profile=digest(canonical(profile)), platform=platform.platform(),
                python=platform.python_version(), memory=memory_state())


class FixtureBackend:
    runtime_version = 'byte-token-fixture/1'
    def __init__(self, protocol):
        self.protocol = protocol
        self.tokenizer = ByteTokenizer()
        self.eos_ids = (256,)
        self.dtype = None
    def load(self, dtype):
        self.dtype = dtype
        return {'requested_dtype': dtype, 'actual_dtype': 'synthetic-byte-tokens', 'runtime_version': self.runtime_version,
                'model_loaded': False, 'torch': None, 'transformers': None, 'threads': None}
    def unload(self):
        self.dtype = None
        gc.collect()
    def prepare(self, messages):
        rendered = self.tokenizer.apply_chat_template(messages)
        tokens = self.tokenizer.encode(rendered)
        return {'rendered_chat': rendered, 'input_ids': tokens, 'attention_mask': [1] * len(tokens), 'inputs': None}
    def effective_config(self):
        return {'do_sample': False, 'num_beams': 1, 'num_return_sequences': 1, 'max_new_tokens': self.protocol['output_cap'],
                'eos_token_id': list(self.eos_ids), 'synthetic': True}
    def generate(self, prepared, candidates, trie):
        # Deliberately always choose pass. Structured construction does not make
        # cal-repair correct; synthetic outcomes must never imply model efficacy.
        text = candidates[0] if trie else '```json\n' + candidates[0] + '\n```'
        tokens = self.tokenizer.encode(text) + [self.eos_ids[0]]
        require(len(tokens) <= self.protocol['output_cap'], 'fixture output exceeds cap')
        return {'text': text, 'completion_ids': tokens, 'generation_ns': None,
                'effective_generation_config': prepared['effective_generation_config']}


class LiveBackend:
    def __init__(self, protocol):
        # Imports and every model load are confined to explicit --live execution.
        import torch
        import transformers
        self.torch = torch
        self.transformers = transformers
        self.protocol = protocol
        require(torch.__version__ == protocol['required_torch'], 'live torch version differs from frozen runtime')
        require(transformers.__version__ == protocol['required_transformers'], 'live transformers version differs from frozen runtime')
        torch.set_num_threads(protocol['threads'])
        torch.manual_seed(0)
        self.runtime_version = canonical({
            'packages': {package: importlib.metadata.version(package) for package in
                         ('torch', 'transformers', 'tokenizers', 'huggingface_hub', 'safetensors', 'accelerate', 'jinja2', 'numpy')},
            'threads': torch.get_num_threads(), 'interop_threads': torch.get_num_interop_threads(),
            'cpu_capability': torch.backends.cpu.get_cpu_capability(),
            'mkldnn_available': torch.backends.mkldnn.is_available(), 'mkldnn_enabled': torch.backends.mkldnn.enabled,
            'torch_build_sha256': digest(torch.__config__.show()), 'python': platform.python_version()})
        self.model = None
        self.tokenizer = None
        self.dtype = None
        self.eos_ids = ()
    def load(self, dtype):
        require(self.model is None, 'previous model must be unloaded before loading another dtype')
        source = dict(revision=self.protocol['revision'], trust_remote_code=False, token=False)
        if self.tokenizer is None:
            self.tokenizer = self.transformers.AutoTokenizer.from_pretrained(self.protocol['model'], **source)
        self.model = self.transformers.AutoModelForCausalLM.from_pretrained(
            self.protocol['model'], torch_dtype=getattr(self.torch, dtype), low_cpu_mem_usage=True,
            use_safetensors=True, **source).eval()
        parameter_dtypes = sorted({str(parameter.dtype) for parameter in self.model.parameters()})
        require(parameter_dtypes == ['torch.' + dtype], 'model dtype differs from requested dtype; no automatic fallback')
        require({parameter.device.type for parameter in self.model.parameters()} == {'cpu'}, 'only the CPU backend is allowed')
        self.dtype = dtype
        eos = self.model.generation_config.eos_token_id
        self.eos_ids = tuple([eos] if type(eos) is int else eos or [])
        require(self.eos_ids, 'model generation config has no EOS token')
        return {'requested_dtype': dtype, 'actual_dtype': parameter_dtypes, 'runtime_version': self.runtime_version,
                'model_loaded': True, 'torch': self.torch.__version__, 'transformers': self.transformers.__version__,
                'threads': self.torch.get_num_threads(), 'interop_threads': self.torch.get_num_interop_threads(),
                'torch_build': self.torch.__config__.show(),
                'cpu_capability': self.torch.backends.cpu.get_cpu_capability(),
                'mkldnn_available': self.torch.backends.mkldnn.is_available(),
                'mkldnn_enabled': self.torch.backends.mkldnn.enabled,
                'model_config': self.model.config.to_dict(), 'base_generation_config': self.model.generation_config.to_dict(),
                'tokenizer_class': type(self.tokenizer).__name__, 'chat_template': self.tokenizer.chat_template,
                'tokenizer_revision_requested': self.protocol['revision'],
                'packages': {package: importlib.metadata.version(package) for package in
                             ('torch', 'transformers', 'tokenizers', 'huggingface_hub', 'safetensors', 'accelerate', 'jinja2', 'numpy')}}
    def unload(self):
        self.model = None
        self.dtype = None
        gc.collect()
    def prepare(self, messages):
        rendered = self.tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
        inputs = self.tokenizer.apply_chat_template(messages, tokenize=True, add_generation_prompt=True,
                                                   return_tensors='pt', return_dict=True)
        return {'rendered_chat': rendered, 'input_ids': inputs['input_ids'][0].tolist(),
                'attention_mask': inputs['attention_mask'][0].tolist(), 'inputs': inputs}
    def effective_config(self):
        config = self.transformers.GenerationConfig.from_dict(self.model.generation_config.to_dict())
        config.do_sample = False
        config.num_beams = 1
        config.num_return_sequences = 1
        config.max_new_tokens = self.protocol['output_cap']
        return config.to_dict()
    def generate(self, prepared, candidates, trie):
        config = self.transformers.GenerationConfig.from_dict(prepared['effective_generation_config'])
        kwargs = {} if trie is None else {'prefix_allowed_tokens_fn': trie.callback(prepared['input_ids'])}
        start = time.monotonic_ns()
        with self.torch.inference_mode():
            ids = self.model.generate(**prepared['inputs'], generation_config=config, **kwargs)
        duration = time.monotonic_ns() - start
        require(ids.shape[0] == 1, 'unexpected number of generated sequences')
        require(ids[0, :len(prepared['input_ids'])].tolist() == prepared['input_ids'], 'generated sequence changed the input prefix')
        completion = ids[0, len(prepared['input_ids']):].tolist()
        text = self.tokenizer.decode(completion, skip_special_tokens=True, clean_up_tokenization_spaces=False)
        return {'text': text, 'completion_ids': completion, 'generation_ns': duration,
                'effective_generation_config': config.to_dict()}


class Journal:
    def __init__(self, path, value):
        self.path = Path(path)
        self.value = value
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open('x', encoding='utf-8') as output:
            json.dump(value, output, ensure_ascii=False, indent=2, allow_nan=False)
            output.write('\n')
            output.flush()
            os.fsync(output.fileno())
    def save(self):
        temporary = Path(str(self.path) + '.writing')
        with temporary.open('x', encoding='utf-8') as output:
            json.dump(self.value, output, ensure_ascii=False, indent=2, allow_nan=False)
            output.write('\n')
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, self.path)
    def event(self, phase, **fields):
        self.value['events'].append(dict(phase=phase, **fields))
        self.save()


def profile_for(protocol, backend, machine, cell, generation_config):
    return {'model': protocol['model'], 'revision': protocol['revision'], 'tokenizer_revision': protocol['revision'],
            'runtime_version': backend.runtime_version, 'dtype': cell['dtype'],
            'decoding': canonical({'method': 'candidate_trie/1' if cell['format'] == 'candidate_trie' else 'free',
                                   'generation_config': generation_config}),
            'cpu_profile': machine['cpu_profile'], 'threads': protocol['threads'],
            'prompt_sha256': protocol['prompt_sha256'], 'output_cap': protocol['output_cap']}


def latency_summaries(calls):
    groups = {}
    for call in calls:
        if call['status'] != 'complete':
            continue
        key = canonical(call['profile'])
        groups.setdefault(key, []).append(call)
    return [{'profile': json.loads(key), 'max_duration_ms': max(math.ceil(c['generation_to_validation_ns'] / 1_000_000) for c in group),
             'sample_count': len(group), 'observed_durations_ms': [c['generation_to_validation_ns'] / 1_000_000 for c in group],
             'max_observed_input_tokens': max(c['input_tokens'] for c in group),
             'max_observed_output_tokens': max(c['output_tokens'] for c in group),
             'includes': 'generation, decoding, raw-response persistence, strict validation and constraint checks',
             'excludes': 'model/tokenizer loading, preparation, output report finalization',
             'warning': 'Maximum of two observed calibration calls, not an upper bound or reliability guarantee.'}
            for key, group in groups.items()]



def admission_diagnostics(summaries, protocol):
    """Pure prospective examples, never dispatch permission or historical rescoring.

    A fixed hypothetical time window is used so no invented second-preparation
    timing is labeled as measured. The quote covers the measured input-size
    envelope, not a newly prepared feedback request. The model is never called.
    """
    from recovery_admission import RuntimeProfile, build_latency_estimate, decide_admission
    diagnostics = []
    for summary in summaries:
        profile = RuntimeProfile.from_dict(summary['profile'])
        estimate = build_latency_estimate(profile, summary['observed_durations_ms'],
                                          max_input_tokens=summary['max_observed_input_tokens'])
        for now_ms in (0, 55000):
            decision = decide_admission(profile=profile, estimate=estimate, operation_start_ms=0,
                                        now_ms=now_ms, deadline_ms=60000, safety_margin_ms=1000,
                                        input_tokens=estimate.max_input_tokens, output_cap=profile.output_cap,
                                        budget_limit=protocol['budget_tokens_per_call'], spent_tokens=0,
                                        reserved_tokens=0, initial_disposition='invalid_output')
            diagnostics.append({'kind': 'hypothetical_post_preparation_window',
                                'estimate_source': 'this calibration profile; same samples, not validation',
                                'new_generation_calls': 0, 'decision': decision.to_dict(),
                                'warning': 'Illustration only. No actual invalid initial response, fresh retry quote, Store admission or dispatch is asserted.'})
    return diagnostics

def run(output, live=False, backend_factory=None, clock=time.monotonic_ns):
    protocol = load_protocol()
    prompt = parse_json(PROMPT_PATH.read_text())
    plan = call_plan(protocol)
    machine = machine_metadata()
    origin = clock()
    value = {'artifact_version': 1, 'protocol_id': protocol['protocol_id'], 'status': 'running',
             'mode': 'live_controlled_calibration' if live else 'synthetic_calibration_fixture',
             'new_model_calls': 0, 'confirmed_model_calls': 0, 'potential_model_calls': 0,
             'unresolved_backend_calls': 0, 'backend_calls_started': 0, 'backend_calls_completed': 0,
             'manifest': {'protocol': protocol, 'protocol_sha256': digest(PROTOCOL_PATH.read_bytes()),
                 'protocol_utf8': PROTOCOL_PATH.read_text(), 'prompt': prompt, 'prompt_utf8': PROMPT_PATH.read_text(), 'machine': machine,
                 'source_utf8': Path(__file__).read_text(), 'call_plan': plan,
                 'admission_helper_source_utf8': Path(__file__).with_name('recovery_admission.py').read_text(),
                 'data_scope': 'Only cal-pass and cal-repair; no heldout corpus or prior replies are loaded.',
                 'structured_semantics': 'Evidence-derived release/tests plus exactly two status candidates. Model chooses pass/fail; no reference label enters generation. Candidate reduction and deterministic extraction are confounds, not pure prompt improvement.',
                 'order_limitations': 'One machine/job, fixed dtype blocks, no automatic warmups. First-after-load and order recorded. Eight calls do not isolate all cold/cache/order effects or establish performance reliability.',
                 'crash_accounting': 'A durable started cell has generation_invoked=null until a returned response or a known-unsent cancellation resolves it. Backend entry is not proof of native model entry. Exceptions without a response retain unresolved_backend_calls=1, unknown usage and new_model_calls=null in live mode; potential_model_calls is an upper bound. Never infer zero calls from a crash.',
                 'deadline_limitations': 'Refuse new loads/calls after 1500s. An already-running native operation is not preempted by this script; the workflow job must enforce the hard process limit.',
                 'fixture_limitations': 'Scripted outputs always choose pass. Byte tokens and fixture timings are not model measurements.'},
             'events': [], 'calls': [], 'latency_calibration': []}
    journal = Journal(output, value)
    backend = None
    loaded_dtype = None
    cases = {case['case_id']: case for case in protocol['cases']}
    def before_work():
        require(clock() - origin < protocol['walltime_seconds'] * 1_000_000_000, 'calibration walltime exhausted')
    try:
        if live:
            require(type(machine['cpu_model']) is str and bool(machine['cpu_model'].strip())
                    and bool(machine['flags']) and bool(machine['affinity']),
                    'known CPU model, flags and affinity are required for live calibration')
        backend = (backend_factory or (LiveBackend if live else FixtureBackend))(protocol)
        for cell in plan:
            before_work()
            if loaded_dtype != cell['dtype']:
                if loaded_dtype is not None:
                    backend.unload()
                    journal.event('unloaded', dtype=loaded_dtype, memory=memory_state())
                journal.event('load_started', dtype=cell['dtype'], memory=memory_state(), operation_ns=clock() - origin)
                before_work()
                load_start = clock()
                metadata = backend.load(cell['dtype'])
                load_ns = clock() - load_start
                loaded_dtype = cell['dtype']
                journal.event('loaded', dtype=loaded_dtype, load_ns=load_ns, metadata=metadata,
                              memory=memory_state(), operation_ns=clock() - origin)
            before_work()
            case = cases[cell['case_id']]
            preparation_start = clock()
            candidates = structured_candidates(case['reports'], case['admitted'])
            payload = canonical({'ci_reports': case['reports']})
            messages = [{'role': 'system', 'content': prompt['system_prompt']}, {'role': 'user', 'content': payload}]
            prepared = backend.prepare(messages)
            prepared['effective_generation_config'] = backend.effective_config()
            require(prepared['input_ids'] == backend.tokenizer.encode(prepared['rendered_chat'], add_special_tokens=False),
                    'rendered chat does not reproduce prepared input token IDs')
            require(prepared['input_ids'] and len(prepared['input_ids']) == len(prepared['attention_mask']), 'invalid prepared input')
            require(all(type(t) is int and t >= 0 for t in prepared['input_ids']), 'invalid prompt token IDs')
            trie = CandidateTrie(backend.tokenizer, candidates, backend.eos_ids, protocol['output_cap']) if cell['format'] == 'candidate_trie' else None
            input_tokens = len(prepared['input_ids'])
            require(input_tokens + protocol['output_cap'] <= protocol['budget_tokens_per_call'], 'exact quote exceeds per-call budget')
            record = dict(cell, status='prepared', profile=profile_for(protocol, backend, machine, cell, prepared['effective_generation_config']),
                          messages=messages, request_utf8=canonical({'messages': messages, 'model': protocol['model'],
                              'revision': protocol['revision'], 'dtype': cell['dtype'], 'decoding': cell['format'],
                              'generation': {'do_sample': False, 'num_beams': 1, 'max_new_tokens': protocol['output_cap']}}),
                          rendered_chat=prepared['rendered_chat'], input_ids=prepared['input_ids'], attention_mask=prepared['attention_mask'],
                          input_ids_sha256=digest(canonical(prepared['input_ids'])), rendered_chat_sha256=digest(prepared['rendered_chat']),
                          input_tokens=input_tokens, output_cap=protocol['output_cap'],
                          effective_generation_config=prepared['effective_generation_config'],
                          candidate_strings=candidates if trie else None, candidate_token_ids=[list(t) for t in trie.token_paths] if trie else None,
                          eos_token_ids=list(backend.eos_ids), preparation_ns=clock() - preparation_start)
            value['calls'].append(record)
            journal.event('prepared', ordinal=cell['ordinal'], operation_ns=clock() - origin)
            before_work()
            require(value['backend_calls_started'] < protocol['max_calls'], 'hard eight-call limit reached')
            record['status'] = 'started'
            record['generation_invoked'] = None
            record['backend_invoked'] = None
            value['backend_calls_started'] += 1
            value['unresolved_backend_calls'] = 1
            value['potential_model_calls'] = value['confirmed_model_calls'] + int(live)
            value['new_model_calls'] = None if live else 0
            # Durable before invocation; errors/crashes are never automatic resend
            # permission. A started but incomplete cell has unknown usage.
            journal.event('generation_started', ordinal=cell['ordinal'], operation_ns=clock() - origin)
            try:
                before_work()  # journal fsync may itself consume the last remaining time
            except Exception:
                record['generation_invoked'] = False
                record['backend_invoked'] = False
                value['unresolved_backend_calls'] = 0
                value['potential_model_calls'] = value['confirmed_model_calls']
                value['new_model_calls'] = value['confirmed_model_calls']
                raise
            record['backend_invoked'] = True
            generation_start = clock()
            reply = backend.generate(prepared, candidates, trie)
            record['raw_response'] = reply
            record['generation_invoked'] = True
            value['confirmed_model_calls'] += int(live)
            value['new_model_calls'] = value['confirmed_model_calls']
            value['unresolved_backend_calls'] = 0
            journal.event('received', ordinal=cell['ordinal'], operation_ns=clock() - origin)
            completion = reply['completion_ids']
            require(reply['effective_generation_config'] == prepared['effective_generation_config'], 'generation settings changed after preflight')
            require(type(completion) is list and all(type(t) is int and t >= 0 for t in completion), 'invalid completion IDs')
            require(0 < len(completion) <= protocol['output_cap'], 'completion usage outside cap')
            require(not set(completion[:-1]).intersection(backend.eos_ids), 'completion continued after EOS')
            ended_eos = completion[-1] in backend.eos_ids
            require(ended_eos or len(completion) == protocol['output_cap'], 'unexplained early generation termination')
            if trie:
                trie.verify(completion, reply['text'])
            correct = strict_validate(reply['text'], case['reference'])
            receipt_ns = clock()
            record.update(status='complete', correct=correct, output_tokens=len(completion), charged_tokens=input_tokens + len(completion),
                          ended_eos=ended_eos, hit_cap=len(completion) == protocol['output_cap'],
                          generation_to_validation_ns=receipt_ns - generation_start,
                          case_elapsed_ns=receipt_ns - preparation_start, operation_ns=receipt_ns - origin)
            value['backend_calls_completed'] += 1
            journal.event('validated', ordinal=cell['ordinal'], correct=correct, operation_ns=clock() - origin)
        backend.unload()
        journal.event('unloaded', dtype=loaded_dtype, memory=memory_state())
        loaded_dtype = None
        value['latency_calibration'] = latency_summaries(value['calls'])
        value['admission_diagnostics'] = admission_diagnostics(value['latency_calibration'], protocol)
        value['status'] = 'complete'
        value['operation_ns'] = clock() - origin
        journal.save()
        return value
    except Exception as error:
        value['status'] = 'failed'
        value['error'] = f'{type(error).__name__}: {error}'
        value['latency_calibration'] = latency_summaries(value['calls'])
        try:
            journal.save()
        except OSError as journal_error:
            print(f'Could not persist final failure: {journal_error}', file=sys.stderr)
        raise
    finally:
        if backend is not None:
            try:
                backend.unload()  # also releases a model assigned before load validation failed
            except Exception as cleanup_error:
                print(f'Backend cleanup failed: {cleanup_error}', file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('recovery-calibration-run/calibration.json'))
    parser.add_argument('--live', action='store_true', help='explicitly run the pinned CPU model; may consume paid compute')
    args = parser.parse_args()
    try:
        artifact = run(args.output, args.live)
    except Exception as error:
        parser.exit(2, f'error: {error}; no automatic retry; inspect {args.output}\n')
    print(json.dumps({'status': artifact['status'], 'mode': artifact['mode'], 'output': str(args.output),
                      'new_model_calls': artifact['new_model_calls'], 'completed_calls': artifact['backend_calls_completed']}))


if __name__ == '__main__':
    main()
