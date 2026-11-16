"""One-shot hosted runner. Only fixture/audit CLI; no live launch switch.

Live execution is a library operation requiring a separately obtained, trusted
approval and billing-bound attestation. Neither artifact JSON nor an environment
variable grants that authority. No live operation is performed by the tests.
"""
import argparse
import base64
from dataclasses import dataclass, asdict
import hashlib
import json
import multiprocessing
import os
from pathlib import Path
import signal
import sqlite3
import time

from incident_adaption_backend import render_case, parse_response
from incident_adaption_transport import send_https, MAX_BODY
from incident_routing import canonical, _loads, require, FREEZE_SHA256

VERSION = 'incident-hosted-single-request/1'

@dataclass(frozen=True)
class LiveApproval:
    """Trusted caller's attestation, not a cryptographic authorization token.

    A human/operator must approve this exact request and the monetary ceiling
    outside this library. Merely constructing this object is not that approval.
    """
    request_sha256: str
    model: str
    maximum_micro_usd: int
    approval_reference: str
    billing_bound_reference: str
    credential_use_approved: bool = False
    data_transmission_approved: bool = False
    all_billable_categories_bounded: bool = False
    currency: str = 'USD'
    bound_basis: str = 'all_in_request_ceiling'
    maximum_duration_ms: int = 60_000


def _now():
    require(hasattr(time, 'CLOCK_BOOTTIME'), 'Linux boottime required')
    return time.clock_gettime_ns(time.CLOCK_BOOTTIME)


def _sources():
    return {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
            for name in ('incident_hosted_runner.py', 'incident_adaption_transport.py',
                         'incident_adaption_backend.py', 'incident_routing.py',
                         'incident_calibration_process.py')}


def _write(path, payload):
    # Exclusive immutable artifact; parent and child never target the same file.
    temporary = path.with_name(path.name + '.tmp')
    with temporary.open('xb') as handle:
        handle.write(canonical(payload).encode() + b'\n')
        handle.flush()
        os.fsync(handle.fileno())
    if path.exists():
        raise FileExistsError(path)
    os.rename(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _db(root):
    db = sqlite3.connect(Path(root) / 'ledger.sqlite', timeout=1)
    db.execute('PRAGMA journal_mode=DELETE')
    db.execute('PRAGMA synchronous=FULL')
    return db


def _validate_approval(approval, rendered, budget):
    if type(approval) is not LiveApproval:
        raise PermissionError('separate live approval and billing attestation required')
    require(approval.request_sha256 == rendered['request_sha256']
            and approval.model == rendered['model'], 'approval request binding')
    require(type(approval.maximum_micro_usd) is int
            and 0 < approval.maximum_micro_usd <= budget, 'approved monetary bound')
    require(approval.currency == 'USD' and approval.bound_basis == 'all_in_request_ceiling',
            'only explicit all-in USD request ceilings are supported')
    require(type(approval.maximum_duration_ms) is int
            and 100 <= approval.maximum_duration_ms <= 1_200_000, 'approved time bound')
    require(all(type(v) is str and bool(v.strip()) and len(v) <= 512 for v in
                (approval.approval_reference, approval.billing_bound_reference)),
            'approval and conservative billing-bound references required')
    require(approval.credential_use_approved is True
            and approval.data_transmission_approved is True
            and approval.all_billable_categories_bounded is True,
            'credential/data/billing scope not approved')


def _persistor(root):
    sequence = 0
    def persist(kind, value):
        nonlocal sequence
        payload = {'sequence': sequence, 'kind': kind,
                   'value': base64.b64encode(value).decode() if kind == 'chunk' else value}
        _write(root / f'transport-{sequence:05d}.json', payload)
        sequence += 1
    return persist


def _child(root, request, mode, deadline_ns, parent_pid, fault):
    # Forked child has no unrelated process group; Linux parent-death protection
    # is armed before any potential network or credential operation.
    from incident_calibration_process import arm_parent_death
    arm_parent_death(parent_pid)
    persist = _persistor(root)
    if _now() >= deadline_ns:
        persist('error', {'kind': 'DeadlineBeforeSend'})
        return
    if mode == 'offline_process_fixture':
        if fault == 'hang':
            while True:
                time.sleep(1)
        if fault == 'crash':
            os._exit(7)
        value = _loads(request)
        body = canonical({'id': 'fixture-response', 'object': 'response',
                          'model': value['model'], 'status': 'completed',
                          'output': [{'type': 'message', 'role': 'assistant',
                                      'content': [{'type': 'output_text', 'text': '{}'}]}],
                          'output_text': '{}',
                          'usage': {'input_tokens': 10, 'output_tokens': 5,
                                    'total_tokens': 15}}).encode()
        persist('headers', {'status': 200, 'headers': []})
        persist('chunk', body if fault != 'partial' else body[:50])
        if fault == 'partial':
            os._exit(8)
        persist('complete', {'status': 200})
        return
    # Not reached by offline tests or CLI. The parent validated exact approval
    # before forking; reading a credential is deferred until this point.
    try:
        key = os.environ['ADAPTION_API_KEY']
        deadline = time.monotonic() + max(0, (deadline_ns - _now()) / 1e9)
        send_https(request, key, deadline=deadline, persist=persist)
    except Exception as error:
        persist('error', {'kind': type(error).__name__})


def _run(root, case_id, model, *, mode, budget_micro_usd, duration_ms,
         approval=None, fault='none'):
    start = _now()
    require(type(duration_ms) is int and 100 <= duration_ms <= 1_200_000, 'bounded duration')
    require(type(budget_micro_usd) is int and 0 < budget_micro_usd <= 2**63 - 1, 'budget')
    require(mode in ('offline_process_fixture', 'live_https'), 'mode')
    require(fault in ('none', 'hang', 'crash', 'partial'), 'fixture fault')
    require(mode == 'offline_process_fixture' or fault == 'none', 'no live faults')
    rendered = render_case(case_id, model)
    if mode == 'live_https':
        _validate_approval(approval, rendered, budget_micro_usd)
        require(duration_ms <= approval.maximum_duration_ms, 'duration exceeds approval')
        hold = approval.maximum_micro_usd
    else:
        require(approval is None, 'fixture cannot consume live authority')
        hold = budget_micro_usd
    deadline = start + duration_ms * 1_000_000
    root = Path(root).resolve()
    root.mkdir(mode=0o700)
    manifest = {'version': VERSION, 'mode': mode, 'request_sha256': rendered['request_sha256'],
                'case_id': case_id, 'model': model, 'freeze_sha256': FREEZE_SHA256,
                'sources': _sources(), 'start_ns': start, 'deadline_ns': deadline,
                'maximum_calls': 1, 'budget_micro_usd': budget_micro_usd,
                'reserved_micro_usd': hold, 'monetary_basis': 'approved_conservative_bound' if mode == 'live_https' else 'synthetic_fixture_bound',
                'approval': asdict(approval) if approval else None,
                'remote_cancellation_guaranteed': False, 'resume_allowed': False}
    _write(root / 'manifest.json', manifest)
    _write(root / 'request.json', {'body_base64': base64.b64encode(rendered['request_bytes']).decode()})
    db = _db(root)
    db.executescript('CREATE TABLE claim(id INTEGER PRIMARY KEY CHECK(id=1),request_sha256 TEXT NOT NULL,tick INTEGER NOT NULL,hold INTEGER NOT NULL); CREATE TRIGGER immutable_claim BEFORE UPDATE ON claim BEGIN SELECT RAISE(ABORT,"immutable claim");END;')
    claimed = False
    process = None
    reason = None
    try:
        # SQLite is the durable single-use claim. Reusing a directory is denied
        # before this point; no restart path can automatically send again.
        with db:
            tick = _now()
            require(tick < deadline, 'deadline before claim')
            db.execute('INSERT INTO claim VALUES(1,?,?,?)', (rendered['request_sha256'], tick, hold))
        claimed = True
        require(_now() < deadline, 'deadline after claim')
        process = multiprocessing.get_context('fork').Process(
            target=_child, args=(root, rendered['request_bytes'], mode, deadline, os.getpid(), fault))
        process.start()
        while process.is_alive() and _now() < deadline:
            process.join(timeout=min(.02, max(0, (deadline - _now()) / 1e9)))
        if process.is_alive():
            reason = 'local_watchdog_expired_remote_outcome_unknown'
            process.kill()
        process.join(timeout=1)
        require(not process.is_alive(), 'child termination not confirmed')
        if reason is None and process.exitcode != 0:
            reason = 'child_exit_without_complete_receipt'
    except Exception as error:
        reason = type(error).__name__
    finally:
        # A caller may catch KeyboardInterrupt/SystemExit and keep this parent
        # alive, so Linux parent-death protection alone is insufficient. Reap
        # the local child on every exit while preserving interrupt propagation
        # and the durable potentially-sent claim/hold.
        try:
            if process is not None and process.pid is not None:
                if process.is_alive():
                    process.kill()
                process.join(timeout=1)
                require(not process.is_alive(), 'child termination not confirmed')
        finally:
            db.close()
    _write(root / 'parent.json', {'claimed': claimed, 'reason': reason,
                                'finished_ns': _now(),
                                'child_exitcode': process.exitcode if process is not None and process.pid else None})
    return audit_run(root)


def run_fixture(root, *, duration_ms=1000, fault='none'):
    return _run(root, 'cal-01', 'fixture-hosted-model/1', mode='offline_process_fixture',
                budget_micro_usd=400, duration_ms=duration_ms, fault=fault)


def run_live(root, case_id, model, *, approval=None, budget_micro_usd,
             duration_ms=60_000):
    """No CLI exposure. Caller must obtain exact external approval first.

    The asserted bound must cover every billable category, even if the client
    dies. This code cannot verify provider prices or enforce provider billing.
    """
    return _run(root, case_id, model, mode='live_https', approval=approval,
                budget_micro_usd=budget_micro_usd, duration_ms=duration_ms)


def audit_run(root):
    root = Path(root).resolve()
    result = {'version': VERSION, 'audit_complete': False, 'provider_reported_usage': None,
              'verified_provider_cost': None, 'held_micro_usd': None,
              'remote_cancellation_guaranteed': False}
    try:
        m = _loads((root / 'manifest.json').read_bytes())
        require(m['version'] == VERSION and m['sources'] == _sources(), 'source identity')
        require(m['mode'] in ('offline_process_fixture', 'live_https'), 'manifest mode')
        rendered = render_case(m['case_id'], m['model'])
        require(rendered['request_sha256'] == m['request_sha256'] and m['freeze_sha256'] == FREEZE_SHA256, 'request provenance')
        raw = base64.b64decode(_loads((root / 'request.json').read_bytes())['body_base64'], validate=True)
        require(raw == rendered['request_bytes'], 'frozen request mismatch')
        require(type(m['start_ns']) is int and type(m['deadline_ns']) is int
                and 100_000_000 <= m['deadline_ns'] - m['start_ns'] <= 1_200_000_000_000, 'deadline window')
        require(type(m['maximum_calls']) is int and m['maximum_calls'] == 1
                and m['resume_allowed'] is False
                and type(m['budget_micro_usd']) is int
                and type(m['reserved_micro_usd']) is int
                and 0 < m['reserved_micro_usd'] <= m['budget_micro_usd'] <= 2**63 - 1, 'bound manifest')
        if m['mode'] == 'live_https':
            _validate_approval(LiveApproval(**m['approval']), rendered, m['budget_micro_usd'])
            require(m['deadline_ns'] - m['start_ns'] <= m['approval']['maximum_duration_ms'] * 1_000_000, 'duration exceeds approval')
            require(m['reserved_micro_usd'] == m['approval']['maximum_micro_usd']
                    and m['monetary_basis'] == 'approved_conservative_bound', 'live hold binding')
        else:
            require(m['approval'] is None and m['monetary_basis'] == 'synthetic_fixture_bound', 'fixture authority')
        ledger = root / 'ledger.sqlite'
        with ledger.open('rb') as handle:
            header = handle.read(20)
        require(header[18:20] == b'\x01\x01'
                and not any(Path(str(ledger) + suffix).exists()
                            for suffix in ('-wal', '-shm', '-journal')),
                'only clean rollback-journal snapshots supported')
        db = sqlite3.connect(ledger.as_uri() + '?mode=ro', uri=True)
        db.execute('PRAGMA query_only=ON')
        try:
            claims = db.execute('SELECT request_sha256,tick,hold FROM claim').fetchall()
        finally:
            db.close()
        require(len(claims) <= 1, 'duplicate claim')
        if claims:
            sha, tick, hold = claims[0]
            require(sha == m['request_sha256'] and m['start_ns'] <= tick < m['deadline_ns']
                    and hold == m['reserved_micro_usd'], 'claim binding')
        body = bytearray()
        status = None
        complete = False
        errors = []
        records = sorted(root.glob('transport-*.json'))
        for index, path in enumerate(records):
            require(path.name == f'transport-{index:05d}.json', 'transport sequence gap')
            record = _loads(path.read_bytes())
            require(record['sequence'] == index and not complete, 'transport record order')
            kind, value = record['kind'], record['value']
            if kind == 'headers':
                require(index == 0 and type(value['status']) is int, 'headers order')
                status = value['status']
            elif kind == 'chunk':
                require(status is not None, 'body without headers')
                body.extend(base64.b64decode(value, validate=True))
                require(len(body) <= MAX_BODY, 'body ceiling')
            elif kind == 'complete':
                require(status is not None and value['status'] == status, 'completion status')
                complete = True
            elif kind == 'error':
                require(type(value) is dict and type(value['kind']) is str, 'error record')
                errors.append(value['kind'])
            else:
                raise ValueError('unknown transport record')
        require(not records or claims, 'transport without claim')
        parsed = parse_response(status, bytes(body), expected_model=m['model'], case_id=m['case_id']) if status is not None else None
        seal = _loads((root / 'parent.json').read_bytes()) if (root / 'parent.json').exists() else None
        if seal:
            require(seal['claimed'] == bool(claims) and type(seal['finished_ns']) is int
                    and seal['finished_ns'] >= m['start_ns'], 'parent seal')
        result.update(audit_complete=True, mode=m['mode'], claimed_count=len(claims),
                      new_model_calls=0 if m['mode'] == 'offline_process_fixture' else None,
                      potential_model_calls=len(claims) if m['mode'] == 'live_https' else 0,
                      monetary_basis=m['monetary_basis'], held_micro_usd=m['reserved_micro_usd'] if claims else 0,
                      receipt_complete=complete, usable_protocol_output=bool(complete and not errors and parsed and parsed['protocol_valid']),
                      raw_body_bytes=len(body), parsed=parsed,
                      provider_reported_usage=parsed['usage'] if parsed else None,
                      transport_errors=errors, parent=seal,
                      billing_reconciliation='not_implemented_hold_retained' if claims else 'unclaimed',
                      state='receipt_recorded_hold_retained' if complete else 'delivery_uncertain' if claims else 'not_sent')
    except Exception as error:
        result.update(audit_error=type(error).__name__, held_micro_usd=None)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    fixture = sub.add_parser('fixture')
    fixture.add_argument('--out', type=Path, required=True)
    audit = sub.add_parser('audit')
    audit.add_argument('out', type=Path)
    args = parser.parse_args(argv)
    report = run_fixture(args.out) if args.command == 'fixture' else audit_run(args.out)
    print(canonical(report))
    return 0 if report['audit_complete'] else 1

if __name__ == '__main__':
    raise SystemExit(main())
