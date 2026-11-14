"""Transport-injected incident prepared-request bridge; offline plan CLI only.

No worker launch, inference, tokenizer estimates, Store admission or durable
permission is implemented here. A trusted caller must exclusively own the injected
transport and enforce any separately authorized execution budget.
"""
import argparse
import copy
import hashlib
import re
import threading
from dataclasses import dataclass
from pathlib import Path

from incident_routing import (VERSION, FREEZE_SHA256, canonical, digest, fields,
                              load_workload, prepare, require, validate_output, _loads)

MODEL = 'Qwen/Qwen2.5-1.5B-Instruct'
REVISION = '989aa7980e4cf806f80c7fef2b1adb7bc71aa306'
CAP = 96
MAX_INPUT = 2048
CALIBRATION_INVALID_OUTPUT = '{"decision":"route"}'
FEEDBACK = ('The previous reply did not satisfy the required JSON protocol. '
            'Return only the specified object with allowed fields and supplied IDs. '
            'Do not infer missing evidence.')
BRIDGE_VERSION = 'incident-preflight/1'


class BridgeError(ValueError):
    """A refused/uncertain operation. reply preserves returned data for audit."""
    def __init__(self, message, *, reply=None, outcome='not_sent'):
        super().__init__(message)
        self.reply = copy.deepcopy(reply)
        self.outcome = outcome


@dataclass(frozen=True)
class Ticket:
    handle: str
    request_utf8: str
    request_sha256: str
    input_tokens: int
    output_cap: int


def _integer(value, minimum, maximum, name):
    require(type(value) is int and minimum <= value <= maximum, f'{name}: invalid integer')


def validate_envelope(request):
    require(type(request) is str, 'frozen request must be UTF-8 JSON text')
    request.encode('utf-8')
    value = _loads(request)
    fields(value, {'adapter', 'model', 'revision', 'dtype', 'add_generation_prompt',
                   'generation', 'messages'}, 'worker envelope')
    require(value['adapter'] == 'transformers-cpu/v1' and value['model'] == MODEL
            and value['revision'] == REVISION and value['dtype'] == 'bfloat16'
            and value['add_generation_prompt'] is True, 'incompatible pinned worker envelope')
    generation = value['generation']
    fields(generation, {'do_sample', 'num_beams', 'max_new_tokens'}, 'generation')
    require(generation['do_sample'] is False, 'sampling is disabled')
    _integer(generation['num_beams'], 1, 1, 'num_beams')
    _integer(generation['max_new_tokens'], CAP, CAP, 'max_new_tokens')
    messages = value['messages']
    require(type(messages) is list and len(messages) in (2, 4), 'initial or feedback messages required')
    roles = ['system', 'user'] if len(messages) == 2 else ['system', 'user', 'assistant', 'user']
    for message, role in zip(messages, roles):
        fields(message, {'role', 'content'}, 'message')
        require(message['role'] == role and type(message['content']) is str and message['content'], 'message role/content')
        message['content'].encode('utf-8')
    return value


def build_request(case_id, invalid_output=None):
    """Calibration-only rendering; feedback depends on format invalidity, not gold."""
    workload = load_workload()
    prepared = prepare(workload, 'calibration')
    records = {r['case_id']: r for r in prepared['requests']}
    require(type(case_id) is str and case_id in records, 'only frozen calibration cases are supported')
    record = records[case_id]
    require(record['request'] is not None, 'complete eligible evidence unavailable')
    messages = _loads(record['request'])['messages']
    if invalid_output is not None:
        require(type(invalid_output) is str and bool(invalid_output), 'prior output required')
        invalid_output.encode('utf-8')
        case = next(c for c in workload['cases']['cases'] if c['case_id'] == case_id)
        require(validate_output(invalid_output, case) is None, 'protocol-valid output cannot receive oracle feedback')
        messages.extend([{'role': 'assistant', 'content': invalid_output}, {'role': 'user', 'content': FEEDBACK}])
    request = canonical({'adapter': 'transformers-cpu/v1', 'model': MODEL, 'revision': REVISION,
                         'dtype': 'bfloat16', 'add_generation_prompt': True, 'messages': messages,
                         'generation': {'do_sample': False, 'num_beams': 1, 'max_new_tokens': CAP}})
    validate_envelope(request)
    return request


class PreparedBridge:
    """Single-thread, one-outstanding-ticket client. No automatic retry or reconnect.

    Transport ownership must be exclusive. Never share a worker between clients.
    Worker-reported counts are unverified here; fake counts remain fake. This
    client neither authenticates a worker nor grants durable Store send authority.
    """
    def __init__(self, transport):
        require(callable(transport), 'transport must be callable')
        self._transport = transport
        self._thread = threading.current_thread()
        self._state = 'idle'
        self._ticket = None
        self._binding = None

    @property
    def state(self):
        return self._state

    def _owner(self):
        if threading.current_thread() is not self._thread:
            raise BridgeError('client is single-thread only')

    def _exchange(self, command):
        # Mark uncertainty before calling out, also fencing reentrant callbacks.
        self._state = 'uncertain'
        try:
            return copy.deepcopy(self._transport(copy.deepcopy(command)))
        except Exception as error:
            raise BridgeError('transport outcome uncertain; no retry or reclaim', outcome='uncertain') from error

    def prepare(self, request):
        self._owner()
        if self._state not in ('idle', 'consumed', 'discarded'):
            raise BridgeError('outstanding or uncertain preparation prevents a new request')
        validate_envelope(request)  # Local failure occurs before any transport.
        reply = self._exchange({'op': 'prepare', 'request_utf8': request})
        try:
            fields(reply, {'handle', 'input_tokens', 'max_new_tokens'}, 'prepared reply')
            require(type(reply['handle']) is str and re.fullmatch('[0-9a-f]{32}', reply['handle']), 'invalid preparation handle')
            _integer(reply['input_tokens'], 1, MAX_INPUT, 'prepared input count')
            _integer(reply['max_new_tokens'], CAP, CAP, 'prepared cap')
        except (ValueError, TypeError) as error:
            raise BridgeError(str(error), reply=reply, outcome='uncertain') from error
        ticket = Ticket(reply['handle'], request, hashlib.sha256(request.encode('utf-8')).hexdigest(),
                        reply['input_tokens'], reply['max_new_tokens'])
        self._ticket = ticket
        self._binding = tuple(vars(ticket).values())
        self._state = 'prepared'
        return ticket

    def _check_ticket(self, ticket):
        self._owner()
        if (self._state != 'prepared' or ticket is not self._ticket
                or tuple(vars(ticket).values()) != self._binding):
            raise BridgeError('ticket is foreign, changed, consumed or unavailable')

    def generate(self, ticket):
        self._check_ticket(ticket)
        handle, _, _, input_tokens, output_cap = self._binding
        reply = self._exchange({'op': 'generate', 'handle': handle,
                                'input_tokens': input_tokens, 'max_new_tokens': output_cap})
        try:
            fields(reply, {'text', 'input_tokens', 'output_tokens', 'generation_us', 'hit_cap', 'ended_eos'}, 'generation reply')
            require(type(reply['text']) is str, 'response text must be a string')
            reply['text'].encode('utf-8')
            _integer(reply['input_tokens'], input_tokens, input_tokens, 'measured input count')
            _integer(reply['output_tokens'], 0, output_cap, 'measured output count')
            _integer(reply['generation_us'], 0, 2**63-1, 'generation duration')
            require(type(reply['hit_cap']) is bool and type(reply['ended_eos']) is bool, 'completion flags must be boolean')
            require(reply['hit_cap'] == (reply['output_tokens'] == output_cap), 'cap flag inconsistent')
            require(not reply['ended_eos'] or reply['output_tokens'] > 0, 'EOS requires a generated token')
        except (ValueError, TypeError, UnicodeError) as error:
            raise BridgeError(str(error), reply=reply, outcome='uncertain') from error
        self._state = 'consumed'
        return reply

    def discard(self, ticket):
        self._check_ticket(ticket)
        handle = self._binding[0]
        reply = self._exchange({'op': 'discard', 'handle': handle})
        try:
            fields(reply, {'discarded'}, 'discard acknowledgment')
            require(reply['discarded'] == handle, 'discard was not confirmed')
        except (ValueError, TypeError) as error:
            raise BridgeError(str(error), reply=reply, outcome='uncertain') from error
        self._state = 'discarded'


def calibration_plan():
    """An unexecuted eight-probe proposal, not a calibration/profile or authority."""
    workload = load_workload()
    cases = [c for c in workload['cases']['cases'] if c['split'] == 'calibration']
    schedule = []
    for phase in ('initial', 'fixed_protocol_feedback'):
        for case in cases:
            request = build_request(case['case_id'], CALIBRATION_INVALID_OUTPUT if phase != 'initial' else None)
            schedule.append({'index': len(schedule), 'case_id': case['case_id'], 'phase': phase,
                             'readiness': 'cold_first_generation_after_load' if not schedule else 'warm_after_known_completed_generation',
                             'measurement_planned': True, 'measurement_status': 'not_executed', 'input_tokens': None,
                             'request_utf8': request, 'request_sha256': hashlib.sha256(request.encode()).hexdigest(),
                             'output_cap': CAP, 'worker_group': 'one_exclusively_owned_worker',
                             'continue_requires': 'load_ready' if not schedule else 'previous_generation_known_complete'})
    plan = {'schema_version': 1, 'version': BRIDGE_VERSION, 'workload_version': VERSION,
            'workload_freeze_sha256': FREEZE_SHA256,
            'worker_source_sha256': hashlib.sha256(Path(__file__).with_name('handoff_model.py').read_bytes()).hexdigest(), 'mode': 'offline_calibration_plan',
            'execution_authorized': False, 'new_model_calls': 0, 'profile_status': 'unmeasured',
            'synthetic_feedback_output': CALIBRATION_INVALID_OUTPUT, 'protocol_feedback': FEEDBACK,
            'max_generation_calls': 8, 'max_input_tokens_per_quote': MAX_INPUT, 'output_cap': CAP,
            'max_total_quoted_tokens': 8*(MAX_INPUT+CAP), 'max_job_elapsed_ms': 1_200_000,
            'time_budget_includes': ['model_load', 'tokenizer_load', 'preparation', 'queue', 'generation', 'validation', 'audit'],
            'warmup_calls': 0, 'automatic_retries': 0, 'schedule': schedule,
            'coverage_limits': ['cold initial n=1', 'warm initial n=3', 'warm fixed-feedback n=4',
                                'no cold-feedback coverage', 'no arbitrary feedback coverage',
                                'no production latency profile', 'no heldout cases'],
            'required_future_observations': ['exact prepared input count per request', 'model and tokenizer revision',
                'torch, transformers and tokenizer dependency versions',
                'exact tokenizer and chat-template configuration', 'worker source digest',
                'full effective generation configuration including EOS/PAD/defaults',
                'dtype and decoding', 'CPU model and flags', 'CPU affinity',
                'actual torch thread count', 'cold/warm readiness', 'load and prepare durations',
                'generation-through-validation duration', 'clock domain and retained operation deadline'],
            'controller_required': 'Separate authorized bounded controller with external all-in watchdog; stop on missing bounds or uncertain result. No automatic retry/reclaim. Historical worker thread policy is min(4, available affinity CPUs).'}
    plan['plan_sha256'] = digest(plan)
    return plan


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args(argv)
    plan = calibration_plan()
    with args.out.open('x', encoding='utf-8') as handle:
        handle.write(canonical(plan) + '\n')
    print(canonical({'mode': plan['mode'], 'new_model_calls': 0, 'execution_authorized': False,
                     'proposed_max_calls': plan['max_generation_calls'], 'plan_sha256': plan['plan_sha256']}))
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError, UnicodeError) as error:
        raise SystemExit(f'Offline preflight refused: {error}')
