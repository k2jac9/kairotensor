"""Check offline Store-captured incident requests against the frozen Python renderer."""
import argparse
from pathlib import Path
import re

import incident_routing as routing


def check(report):
    """Compare exact UTF-8 bytes and order, never just producer-supplied digests.

    Capture IDs and manifest digests are producer provenance, not independent
    proof of Store execution or authorization. The Rust adapter tests cover that.
    """
    routing.fields(report, {
        'schema_version', 'version', 'mode', 'new_model_calls',
        'accounting_unit', 'capture', 'freeze_sha256', 'requests',
    }, 'Store request export')
    expected_metadata = {
        'schema_version': 1, 'version': 'incident-store-requests/1',
        'mode': 'synthetic_store_incident_fixture', 'new_model_calls': 0,
        'accounting_unit': 'UTF-8 bytes', 'capture': 'Store',
        'freeze_sha256': routing.FREEZE_SHA256,
    }
    for key, value in expected_metadata.items():
        routing.require(type(report[key]) is type(value) and report[key] == value,
                        f'incorrect {key}')
    workload = routing.load_workload()
    expected = routing.prepare(workload, split='all')['requests']
    records = report['requests']
    routing.require(type(records) is list and len(records) == len(expected), 'case count mismatch')
    for record, wanted, case in zip(records, expected, workload['cases']['cases']):
        routing.fields(record, {'case_id', 'request', 'request_sha256',
                                'manifest_sha256', 'captured_ids'}, 'request record')
        routing.require(record['case_id'] == wanted['case_id'], 'case order mismatch')
        routing.require(type(record['request']) is str and
                        record['request'].encode('utf-8') == wanted['request'].encode('utf-8'),
                        f"request bytes mismatch: {wanted['case_id']}")
        routing.require(record['request_sha256'] == wanted['request_sha256'], 'request digest mismatch')
        digest = record['manifest_sha256']
        routing.require(type(digest) is str and re.fullmatch('[0-9a-f]{64}', digest) is not None,
                        'invalid manifest digest')
        ids = record['captured_ids']
        routing.require(type(ids) is list and len(ids) == len(case['evidence']) + 1 and
                        all(type(i) is int and i > 0 for i in ids) and len(set(ids)) == len(ids),
                        'invalid captured occurrence IDs')
    return len(records)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--requests', required=True, type=Path)
    args = parser.parse_args(argv)
    count = check(routing.read_json(args.requests))
    print(routing.canonical({'mode': 'offline_store_byte_parity', 'cases_checked': count,
                             'new_model_calls': 0, 'accounting_unit': 'UTF-8 bytes'}))
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError, UnicodeError, RecursionError) as error:
        raise SystemExit(f'Store request parity refused: {error}')
