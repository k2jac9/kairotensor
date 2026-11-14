"""Subprocess protocol fixture. Imports RequestHandler but never its live main."""
import argparse
import hashlib
import json
import os
from incident_calibration_process import arm_parent_death
from pathlib import Path
import sys
import time
from types import SimpleNamespace
from handoff_model import RequestHandler
from incident_preflight import MODEL, REVISION, validate_envelope


def now_ns(mode):
    if mode == 'linux_boottime':
        return time.clock_gettime_ns(time.CLOCK_BOOTTIME)
    if mode == 'fixture_monotonic':
        return time.monotonic_ns()
    raise ValueError('unknown fixture clock')


def profile():
    return {'mode': 'offline_protocol_fixture', 'model': MODEL, 'revision': REVISION,
            'dtype': 'bfloat16', 'runtime': 'fake-tensors/1', 'tokenizer': 'fake-shape/1',
            'threads': 1, 'hardware': 'synthetic-not-a-host-profile',
            'generation': {'do_sample': False, 'num_beams': 1, 'max_new_tokens': 96},
            'handler_source_sha256': hashlib.sha256(Path(__file__).with_name('handoff_model.py').read_bytes()).hexdigest(),
            'fixture_source_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--parent-pid', type=int, required=True)
    parser.add_argument('--fault', required=True)
    parser.add_argument('--work-deadline-ns', type=int, required=True)
    parser.add_argument('--clock-mode', required=True)
    args = parser.parse_args()
    arm_parent_death(args.parent_pid)
    ready = profile()
    if args.fault == 'runtime_mismatch':
        ready['revision'] = 'wrong'
    print(json.dumps({'ready': True, 'profile': ready}), flush=True)
    generated = 0

    def tokenize(request):
        validate_envelope(json.dumps(request))
        # Deliberately fake shape; never a real tokenizer count or calibration.
        count = 173 if len(request['messages']) == 2 else 211
        return {'input_ids': SimpleNamespace(shape=(1, count))}, request['generation']

    def generate(inputs, generation):
        nonlocal generated
        generated += 1
        if args.fault == 'worker_eof': os._exit(31)
        if args.fault == 'hang_generation':
            time.sleep(3600)
        if args.fault in ('late_reply','late_receipt_failure') and generated == 1:
            remaining = (args.work_deadline_ns + 20_000_000 - now_ns(args.clock_mode)) / 1e9
            if remaining > 0:
                time.sleep(remaining)
        return {'text': '{"decision":"abstain","owner":null,"reason":"undetermined_cause","evidence_ids":["e01"]}',
                'input_tokens': inputs['input_ids'].shape[1], 'output_tokens': 7,
                'generation_us': 1, 'hit_cap': False, 'ended_eos': True}

    handler = RequestHandler(tokenize, generate)
    for raw in sys.stdin.buffer:
        try:
            command = json.loads(raw)
            result = handler.handle(command)
            if command.get('op') == 'prepare':
                if args.fault == 'wrong_quote': result['input_tokens'] = 2049
                if args.fault == 'partial_line':
                    sys.stdout.write('{"handle":'); sys.stdout.flush(); time.sleep(3600)
                if args.fault == 'oversized_reply':
                    sys.stdout.write('x' * 131073 + '\n'); sys.stdout.flush(); continue
            if command.get('op') == 'generate' and args.fault == 'wrong_usage':
                result['output_tokens'] = 97
            print(json.dumps(result, separators=(',', ':')), flush=True)
        except Exception as error:
            print(json.dumps({'error': str(error)}), flush=True)


if __name__ == '__main__':
    main()
