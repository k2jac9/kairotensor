"""Offline-only bounded diagnostic controller, not Store/production admission."""
import time
# Earliest available script clock, before plan loading/setup. No restart/resume.
_START_MODE = 'linux_boottime' if hasattr(time, 'CLOCK_BOOTTIME') else 'fixture_monotonic'
_START_NS = time.clock_gettime_ns(time.CLOCK_BOOTTIME) if _START_MODE == 'linux_boottime' else time.monotonic_ns()
import argparse
import base64
import copy
from contextlib import closing
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile

from incident_routing import canonical, digest, require, _loads
from incident_preflight import PreparedBridge, BridgeError, calibration_plan, validate_envelope
from incident_calibration_fixture import profile, now_ns
from incident_calibration_process import arm_parent_death

FAULTS = {'none', 'after_reserve', 'after_claim', 'after_receipt', 'hang_before_worker',
          'hang_generation', 'hang_after_receipt', 'partial_line', 'oversized_reply',
          'wrong_quote', 'runtime_mismatch', 'wrong_usage', 'late_reply',
          'sqlite_receipt_failure', 'sqlite_settle_failure', 'receipt_fsync_failure',
          'late_receipt_failure', 'sqlite_claim_failure', 'worker_eof'}

@dataclass(frozen=True)
class ExecutionApproval:
    kind: str = 'deny'


def _sources():
    return {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
            for name in ('incident_calibration_controller.py', 'incident_preflight.py', 'incident_calibration_process.py')}


# Explicit trusted PR16 source identity, not an artifact-provided allowlist.
_LEGACY_V1_SOURCES = {
    'incident_calibration_controller.py':'3ec8c274c42a86e51c2daa1de1eba200fc3779b1d4cbdf7fcad308cafd362c49',
    'incident_preflight.py':'f0955476e7db0d0fe56052c7042ce2820e9630eb6c95b22f34c0b4b328e295e4',
    'incident_calibration_process.py':'92575eeb9f71869bc51009031a05ed4a2a4c6e0a136e8ffedaeb52ff14629962',
}

def _validate_manifest(m):
    require(type(m) is dict and m['version'] == 'incident-calibration-controller/1', 'manifest version')
    require(m['mode'] == 'offline_protocol_fixture' and type(m['new_model_calls']) is int and m['new_model_calls'] == 0, 'offline manifest required')
    require(m['runtime'] == profile() and m['sources'] in (_sources(), _LEGACY_V1_SOURCES), 'runtime/source binding mismatch')
    require(canonical(m['plan']) == canonical(calibration_plan()), 'frozen plan mismatch')
    require(m['clock_mode'] in ('linux_boottime', 'fixture_monotonic'), 'clock mode')
    require(all(type(m[k]) is int for k in ('start_ns','work_deadline_ns','deadline_ns')), 'clock integer required')
    require(0 <= m['start_ns'] < m['work_deadline_ns'] < m['deadline_ns'] and m['deadline_ns']-m['start_ns'] <= 1200000*1000000, 'clock window invalid')
    require(m['fault'] in FAULTS and m['no_resume'] is True and m['accounting'] == 'fake_tensor_protocol_units_not_model_tokens', 'manifest controls invalid')


def _durable_new(path, payload, *, fail_fsync=False):
    data = canonical(payload).encode('utf-8') + b'\n'
    temporary = path.with_name(path.name + '.tmp')
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, 'wb') as handle:
            handle.write(data); handle.flush()
            if fail_fsync: raise OSError('injected receipt fsync failure')
            os.fsync(handle.fileno())
        if path.exists(): raise FileExistsError(path)
        os.rename(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
    except BaseException:
        # A .tmp file is not advertised as durable or used by reconciliation.
        raise


def _connect(path):
    db = sqlite3.connect(path, timeout=0.1)
    try:
        db.execute('PRAGMA journal_mode=DELETE'); db.execute('PRAGMA synchronous=FULL')
    except BaseException:
        db.close()
        raise
    return db


def _event(db, event, slot, payload, clock_mode):
    db.execute('INSERT INTO events(event,slot,payload,observed_ns) VALUES(?,?,?,?)',
               (event, slot, canonical(payload), now_ns(clock_mode)))


def _initialize(out, plan, mode):
    fd = os.open(out / 'ledger.sqlite', os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    db = _connect(out / 'ledger.sqlite')
    try:
        db.executescript('''
          CREATE TABLE slots(slot INTEGER PRIMARY KEY, request_utf8 TEXT NOT NULL, request_sha256 TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('planned','reserved','claimed','received','settled','uncertain')),
            input_units INTEGER,output_cap INTEGER,handle TEXT,charged_units INTEGER,
            raw_reply BLOB,receipt_ns INTEGER,late INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE events(id INTEGER PRIMARY KEY,event TEXT NOT NULL,slot INTEGER,payload TEXT NOT NULL,observed_ns INTEGER NOT NULL);
          CREATE TRIGGER immutable_request BEFORE UPDATE OF request_utf8,request_sha256 ON slots
            BEGIN SELECT RAISE(ABORT,'request binding immutable'); END;
          CREATE TRIGGER immutable_events_update BEFORE UPDATE ON events
            BEGIN SELECT RAISE(ABORT,'events immutable'); END;
          CREATE TRIGGER immutable_events_delete BEFORE DELETE ON events
            BEGIN SELECT RAISE(ABORT,'events immutable'); END;
        ''')
        with db:
            for slot in plan['schedule']:
                db.execute('INSERT INTO slots(slot,request_utf8,request_sha256,state) VALUES(?,?,?,?)',
                           (slot['index'], slot['request_utf8'], slot['request_sha256'], 'planned'))
            _event(db, 'planned', None, {'plan_sha256': plan['plan_sha256'], 'manifest_sha256': digest(_loads((out / 'manifest.json').read_bytes()))}, mode)
    except BaseException:
        db.close()
        raise
    return db


def _accounting(db):
    charged, held = db.execute('''SELECT coalesce(sum(charged_units),0),
       coalesce(sum(CASE WHEN input_units IS NOT NULL AND charged_units IS NULL THEN input_units+output_cap ELSE 0 END),0)
       FROM slots''').fetchone()
    return charged, held


def _reserve(db, slot, ticket, manifest):
    db.execute('BEGIN IMMEDIATE')
    try:
        state, request, sha = db.execute('SELECT state,request_utf8,request_sha256 FROM slots WHERE slot=?', (slot,)).fetchone()
        require(state == 'planned' and request == ticket.request_utf8 and sha == ticket.request_sha256, 'reservation binding mismatch')
        require(type(ticket.input_tokens) is int and 1 <= ticket.input_tokens <= 2048 and ticket.output_cap == 96, 'quote exceeds frozen caps')
        charged, held = _accounting(db)
        require(charged + held + ticket.input_tokens + ticket.output_cap <= 17152, 'total quote budget denied')
        db.execute("UPDATE slots SET state='reserved',input_units=?,output_cap=?,handle=? WHERE slot=?",
                   (ticket.input_tokens, ticket.output_cap, ticket.handle, slot))
        _event(db, 'reserved', slot, {'input_units': ticket.input_tokens, 'output_cap': ticket.output_cap}, manifest['clock_mode'])
        db.commit()
    except BaseException:
        db.rollback(); raise


def _claim(db, slot, ticket, manifest, actual_profile):
    db.execute('BEGIN IMMEDIATE')
    try:
        require(actual_profile == manifest['runtime'], 'runtime identity changed')
        row = db.execute('SELECT state,request_utf8,request_sha256,input_units,output_cap,handle FROM slots WHERE slot=?', (slot,)).fetchone()
        require(row == ('reserved', ticket.request_utf8, ticket.request_sha256, ticket.input_tokens, ticket.output_cap, ticket.handle), 'claim quote mismatch')
        require(slot < 8 and db.execute("SELECT count(*) FROM events WHERE event='claimed'").fetchone()[0] < 8, 'call limit')
        require(db.execute("SELECT count(*) FROM slots WHERE slot<? AND state<>'settled'", (slot,)).fetchone()[0] == 0, 'previous generation is not known complete')
        charged, held = _accounting(db); require(charged + held <= 17152, 'shared budget exceeded')
        # Sample after all database checks while holding the writer lock.
        require(now_ns(manifest['clock_mode']) < manifest['work_deadline_ns'], 'work deadline reached')
        db.execute("UPDATE slots SET state='claimed' WHERE slot=?", (slot,))
        _event(db, 'claimed', slot, {'request_sha256': ticket.request_sha256}, manifest['clock_mode'])
        db.commit()
    except BaseException:
        db.rollback(); raise


def _receipt(db, out, slot, raw, manifest, fault, errors, *, observed_ns=None):
    sampled = now_ns(manifest['clock_mode'])
    observed = sampled if observed_ns is None else observed_ns
    require(type(observed) is int and 0 <= observed <= sampled, 'receipt observation invalid')
    claimed = db.execute("SELECT max(observed_ns) FROM events WHERE event='claimed' AND slot=?", (slot,)).fetchone()[0]
    require(type(claimed) is int and observed >= claimed, 'receipt precedes claim')
    request_sha = manifest['plan']['schedule'][slot]['request_sha256']
    record = {'slot': slot, 'request_sha256': request_sha, 'raw_reply_base64': base64.b64encode(raw).decode(), 'observed_ns': observed}
    # Independent raw artifact precedes parsing/protocol checks and ledger settle.
    try:
        _durable_new(out / f'receipt-{slot}.json', record, fail_fsync=fault == 'receipt_fsync_failure')
    except OSError as error:
        errors.append(f'receipt artifact: {error}')
    try:
        with db:
            prior = db.execute('SELECT raw_reply,state FROM slots WHERE slot=?', (slot,)).fetchone()
            require(prior[1] == 'claimed' and prior[0] is None, 'receipt does not match a fresh claim')
            db.execute("UPDATE slots SET state='received',raw_reply=?,receipt_ns=?,late=? WHERE slot=?",
                       (raw, observed, int(observed >= manifest['work_deadline_ns']), slot))
            _event(db, 'received', slot, {'raw_sha256': hashlib.sha256(raw).hexdigest()}, manifest['clock_mode'])
    except Exception as error:
        errors.append(f'receipt ledger: {error}')
        raise
    if fault == 'after_receipt': os._exit(29)
    if fault == 'hang_after_receipt': time.sleep(3600)


class _Transport:
    def __init__(self, process, manifest, db, out, fault, errors):
        self.process, self.manifest, self.db, self.out, self.fault, self.errors = process, manifest, db, out, fault, errors
        self.slot = None; self.buffer = b''
        os.set_blocking(process.stdout.fileno(), False)
        os.set_blocking(process.stdin.fileno(), False)

    def _line(self, deadline):
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while b'\n' not in self.buffer:
                remaining = (deadline - now_ns(self.manifest['clock_mode'])) / 1e9
                if remaining <= 0: raise TimeoutError('worker reply deadline reached')
                if not selector.select(min(remaining, .05)): continue
                chunk = os.read(self.process.stdout.fileno(), 4096)
                if not chunk: raise EOFError('worker ended without a complete receipt')
                self.buffer += chunk
                if len(self.buffer) > 65536: raise ValueError('worker reply exceeds 64KiB')
            line, self.buffer = self.buffer.split(b'\n', 1)
            return line

    def ready(self):
        return _loads(self._line(self.manifest['work_deadline_ns']))

    def __call__(self, command):
        deadline = self.manifest['work_deadline_ns']
        if command['op'] == 'generate':
            # Final postclaim check, no filesystem I/O before the command write.
            require(now_ns(self.manifest['clock_mode']) < deadline, 'postclaim work deadline reached')
        payload = canonical(command).encode() + b'\n'
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdin, selectors.EVENT_WRITE)
            while payload:
                remaining = (deadline - now_ns(self.manifest['clock_mode'])) / 1e9
                if remaining <= 0: raise TimeoutError('worker send deadline reached')
                if not selector.select(min(remaining, .05)): continue
                require(now_ns(self.manifest['clock_mode']) < deadline, 'send deadline reached')
                sent = os.write(self.process.stdin.fileno(), payload)
                payload = payload[sent:]
        # Leave half of audit reserve for a late known receipt; never another call.
        read_deadline = (self.manifest['deadline_ns'] + self.manifest['work_deadline_ns']) // 2 if command['op'] == 'generate' else deadline
        raw = self._line(read_deadline)
        if command['op'] == 'generate':
            _receipt(self.db, self.out, self.slot, raw, self.manifest, self.fault, self.errors)
        return _loads(raw)


def _child(out, parent_pid):
    arm_parent_death(parent_pid)
    manifest = _loads((out / 'manifest.json').read_bytes()); plan = manifest['plan']; fault = manifest['fault']
    _validate_manifest(manifest)
    require(now_ns(manifest['clock_mode']) < manifest['work_deadline_ns'], 'child started after deadline')
    db = _initialize(out, plan, manifest['clock_mode']); worker = None; errors = []
    try:
        if fault == 'hang_before_worker': time.sleep(3600)
        if fault in ('sqlite_receipt_failure', 'late_receipt_failure'):
            db.execute("CREATE TRIGGER inject_receipt BEFORE UPDATE OF raw_reply ON slots BEGIN SELECT RAISE(ABORT,'injected receipt write failure'); END")
        if fault == 'sqlite_claim_failure':
            db.execute("CREATE TRIGGER inject_claim BEFORE UPDATE OF state ON slots WHEN NEW.state='claimed' BEGIN SELECT RAISE(ABORT,'injected claim failure'); END")
        if fault == 'sqlite_settle_failure':
            db.execute("CREATE TRIGGER inject_settle BEFORE UPDATE OF charged_units ON slots BEGIN SELECT RAISE(ABORT,'injected settle failure'); END")
        worker = subprocess.Popen([sys.executable, str(Path(__file__).with_name('incident_calibration_fixture.py')), '--fault', fault,
                                   '--parent-pid', str(os.getpid()), '--work-deadline-ns', str(manifest['work_deadline_ns']), '--clock-mode', manifest['clock_mode']],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        transport = _Transport(worker, manifest, db, out, fault, errors)
        ready = transport.ready(); require(ready == {'ready': True, 'profile': manifest['runtime']}, 'worker readiness mismatch')
        client = PreparedBridge(transport)
        for slot in plan['schedule']:
            index = slot['index']; transport.slot = index
            require(now_ns(manifest['clock_mode']) < manifest['work_deadline_ns'], 'work deadline reached')
            validate_envelope(slot['request_utf8'])
            ticket = client.prepare(slot['request_utf8'])
            _reserve(db, index, ticket, manifest)
            if fault == 'after_reserve': os._exit(27)
            _claim(db, index, ticket, manifest, ready['profile'])
            if fault == 'after_claim': os._exit(28)
            reply = client.generate(ticket)
            usage = reply['input_tokens'] + reply['output_tokens']
            with db:
                require(db.execute('SELECT state FROM slots WHERE slot=?', (index,)).fetchone()[0] == 'received', 'raw receipt not persisted')
                db.execute("UPDATE slots SET state='settled',charged_units=? WHERE slot=?", (usage, index))
                _event(db, 'settled', index, {'charged_units': usage}, manifest['clock_mode'])
            if errors: break
        with db:
            _event(db, 'controller_finished', None, {'errors': errors}, manifest['clock_mode'])
    except BaseException as error:
        errors.append(f'{type(error).__name__}: {error}')
        try:
            with db:
                db.execute("UPDATE slots SET state='uncertain' WHERE state='claimed'")
                _event(db, 'controller_failed', None, {'errors': errors}, manifest['clock_mode'])
        except Exception: pass
    finally:
        if worker is not None:
            worker.kill()
            try: worker.wait(timeout=.2)
            except subprocess.TimeoutExpired: pass
        db.close()
    # This child audit is still covered by the supervisor's original deadline.
    report = audit_run(out)
    _durable_new(out / 'child-audit.json', {'report': report, 'errors': errors})
    return 0 if not errors else 1


def _valid_receipt(raw, input_units, cap):
    try:
        value = _loads(raw)
        require(type(value) is dict and set(value) == {'text','input_tokens','output_tokens','generation_us','hit_cap','ended_eos'}, 'receipt fields')
        require(type(value['text']) is str, 'receipt text')
        value['text'].encode('utf-8')
        require(type(value['input_tokens']) is int and value['input_tokens'] == input_units, 'input quote mismatch')
        require(type(value['output_tokens']) is int and 0 <= value['output_tokens'] <= cap, 'output cap mismatch')
        require(type(value['generation_us']) is int and 0 <= value['generation_us'] < 2**63, 'duration')
        require(type(value['hit_cap']) is bool and value['hit_cap'] == (value['output_tokens'] == cap), 'cap flag')
        require(type(value['ended_eos']) is bool and (not value['ended_eos'] or value['output_tokens'] > 0), 'EOS flag')
        return value['input_tokens'] + value['output_tokens']
    except (ValueError, TypeError, KeyError, UnicodeError): return None


def audit_run(out, *, _manifest_validator=None, _event_validator=None):
    """Read-only consistency audit, not source authentication or replay permission."""
    out = Path(out)
    empty = {'mode': 'offline_calibration_diagnostic', 'new_model_calls': 0, 'profile_exported': False,
             'complete': False, 'audit_complete': False, 'claimed_slots': 0, 'settled_slots': 0,
             'ledger_charged_units': None, 'unresolved_hold_units': None, 'known_receipt_units': None,
             'late_receipts': 0, 'slots': [], 'reconciliation': [], 'supervisor': {}, 'snapshot_only': True}
    result = copy.deepcopy(empty)
    try:
        manifest = _loads((out / 'manifest.json').read_bytes()); (_manifest_validator or _validate_manifest)(manifest)
        if (out / 'supervisor.json').exists():
            seal = _loads((out / 'supervisor.json').read_bytes())
            require(seal['manifest_sha256'] == digest(manifest) and seal['deadline_ns'] == manifest['deadline_ns'], 'supervisor binding')
            require(seal['child_reaped'] is True and seal['group_terminated'] is True and type(seal['timed_out']) is bool and type(seal['exit_code']) is int, 'supervisor termination invalid')
            require(type(seal['finished_ns']) is int and seal['finished_ns'] >= manifest['start_ns'], 'supervisor time invalid')
            require(type(seal['deadline_overrun']) is bool and seal['deadline_overrun'] == (seal['finished_ns'] >= manifest['deadline_ns']), 'supervisor overrun mismatch')
            result.update(supervisor=seal, snapshot_only=False)
        def snapshot():
            require(not any((out / ('ledger.sqlite'+suffix)).exists() for suffix in ('-wal','-shm')), 'WAL bundles unsupported')
            return {suffix: (out / ('ledger.sqlite'+suffix)).read_bytes() for suffix in ('','-journal') if (out / ('ledger.sqlite'+suffix)).exists()}
        before = snapshot()
        require('' in before and before[''][18:20] == b'\x01\x01', 'rollback journal SQLite required')
        with tempfile.TemporaryDirectory(prefix='incident-audit-copy-') as temporary:
            path = Path(temporary) / 'ledger.sqlite'
            for suffix, data in before.items(): Path(str(path)+suffix).write_bytes(data)
            require(snapshot() == before, 'source changed during audit snapshot')
            with closing(sqlite3.connect(path)) as db:
                rows = db.execute('SELECT slot,request_utf8,request_sha256,state,input_units,output_cap,charged_units,raw_reply,receipt_ns,late FROM slots ORDER BY slot').fetchall()
                events = db.execute('SELECT event,slot,payload,observed_ns FROM events ORDER BY id').fetchall()
        require([r[0] for r in rows] == list(range(8)), 'exact planned slot set required')
        require(events and events[0][0:2] == ('planned', None), 'initial event missing')
        require(_loads(events[0][2]) == {'plan_sha256': manifest['plan']['plan_sha256'], 'manifest_sha256': digest(manifest)}, 'ledger manifest binding')
        if _event_validator is not None:
            events = _event_validator(out, manifest, rows, events, result['supervisor'])
        previous = manifest['start_ns']; chains = {i: [] for i in range(8)}; finished = False; audits_ok = True
        for index, (name, slot, payload, tick) in enumerate(events):
            require(type(tick) is int and tick >= previous, 'event clock regression'); previous = tick
            if not result['snapshot_only']: require(tick <= result['supervisor']['finished_ns'], 'event after supervisor termination')
            value = _loads(payload)
            if slot is None:
                require((index == 0 and name == 'planned') or name in ('controller_finished','controller_failed'), 'unknown global event')
                if index:
                    require(index == len(events)-1 and type(value) is dict and set(value) == {'errors'} and type(value['errors']) is list and all(type(error) is str for error in value['errors']), 'terminal event chain')
                    finished = name == 'controller_finished' and value['errors'] == []
                    if value['errors'] or name == 'controller_failed': audits_ok = False
            else:
                require(type(slot) is int and slot in chains and name in ('reserved','claimed','received','settled'), 'unknown slot event')
                chains[slot].append((name,value,tick))
        charged = held = known = late = claims = settled = 0
        lengths = {'planned':0,'reserved':1,'claimed':2,'uncertain':2,'received':3,'settled':4}
        for slot, request, sha, state, inputs, cap, spent, raw, receipt_ns, is_late in rows:
            expected = manifest['plan']['schedule'][slot]; chain = chains[slot]
            require(request == expected['request_utf8'] and sha == expected['request_sha256'], 'slot request mismatch')
            require(state in lengths and [e[0] for e in chain] == ['reserved','claimed','received','settled'][:lengths[state]], 'slot event/state mismatch')
            require(type(is_late) is int and is_late in (0,1), 'late flag invalid')
            if state == 'planned':
                require(inputs is None and cap is None and spent is None and raw is None and receipt_ns is None and is_late == 0, 'planned accounting invalid')
            else:
                require(type(inputs) is int and 1 <= inputs <= 2048 and type(cap) is int and cap == 96, 'ledger quote invalid')
                require(chain[0][1] == {'input_units':inputs,'output_cap':cap}, 'reservation event mismatch')
            if len(chain) >= 2:
                claims += 1
                require(chain[1][1] == {'request_sha256':sha} and chain[1][2] < manifest['work_deadline_ns'], 'claim event mismatch')
                require(all(r[3] == 'settled' for r in rows[:slot]), 'claim after uncertain prior slot')
            if state == 'settled':
                require(type(spent) is int and spent >= 0 and chain[3][1] == {'charged_units':spent}, 'settled accounting invalid')
                charged += spent; settled += 1
            else:
                require(spent is None, 'unsettled charge invalid')
                if inputs is not None: held += inputs + cap
            if raw is not None:
                require(len(chain) >= 3 and type(raw) is bytes and type(receipt_ns) is int, 'unbound ledger receipt')
                require(chain[1][2] <= receipt_ns <= chain[2][2] and chain[2][1] == {'raw_sha256':hashlib.sha256(raw).hexdigest()}, 'receipt event mismatch')
                require(is_late == int(receipt_ns >= manifest['work_deadline_ns']), 'receipt timing mismatch')
            else: require(receipt_ns is None and is_late == 0, 'receipt timestamp without bytes')
            fallback = out / f'receipt-{slot}.json'
            if fallback.exists():
                record = _loads(fallback.read_bytes())
                require(type(record) is dict and set(record) == {'slot','request_sha256','raw_reply_base64','observed_ns'}, 'fallback fields')
                require(type(record['slot']) is int and record['slot'] == slot and record['request_sha256'] == sha and len(chain) >= 2, 'fallback binding')
                independent = base64.b64decode(record['raw_reply_base64'], validate=True)
                observed = record['observed_ns']
                require(type(observed) is int and observed >= chain[1][2], 'fallback clock invalid')
                require(raw is None or (raw == independent and receipt_ns == observed), 'conflicting durable receipts')
                raw, receipt_ns = independent, observed
            usage = _valid_receipt(raw, inputs, cap) if raw is not None and inputs is not None else None
            if raw is not None:
                late += int(receipt_ns >= manifest['work_deadline_ns'])
                if not result['snapshot_only']: require(receipt_ns <= result['supervisor']['finished_ns'], 'receipt after supervisor termination')
            if usage is not None: known += usage
            if state == 'settled': require(usage == spent, 'settled receipt mismatch')
            elif inputs is not None: result['reconciliation'].append({'slot':slot,'action':'review_known_receipt_no_resend' if usage is not None else 'retain_hold_no_resend','measured_units':usage})
            result['slots'].append({'slot':slot,'state':state,'charged_units':spent,'known_receipt_units':usage})
        require(charged + held <= 17152 and claims <= 8, 'aggregate budget invariant')
        result.update(claimed_slots=claims,settled_slots=settled,ledger_charged_units=charged,unresolved_hold_units=held,known_receipt_units=known,late_receipts=late,audit_complete=audits_ok)
        seal = result['supervisor']
        result['complete'] = settled == 8 and audits_ok and finished and not result['snapshot_only'] and not seal['timed_out'] and not seal['deadline_overrun'] and seal['exit_code'] == 0
    except (ValueError, TypeError, KeyError, OSError, sqlite3.Error, IndexError, OverflowError, RecursionError) as error:
        result = copy.deepcopy(empty); result['audit_error'] = str(error)
    return result

def _supervise(out, command, deadline, mode, manifest):
    with (out / 'controller-stderr.log').open('xb') as stderr:
        child = subprocess.Popen(command,
                                 start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stderr)
        timed_out = False
        try:
            while os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None:
                if now_ns(mode) >= deadline:
                    timed_out = True
                    break
                time.sleep(.01)
        finally:
            # PID remains unreaped, so it cannot be recycled into an unrelated group.
            try:
                require(os.getpgid(child.pid) == child.pid and os.getsid(child.pid) == child.pid, 'owned group identity changed')
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError: pass
            child.wait()
    observed_finish = now_ns(mode)
    _durable_new(out / 'supervisor.json', {'timed_out': timed_out, 'deadline_overrun': observed_finish >= deadline, 'child_reaped': True, 'exit_code': child.returncode,
                                          'finished_ns': observed_finish, 'deadline_ns': deadline, 'manifest_sha256': digest(manifest), 'group_terminated': True})


def run_fixture(out, *, approval=None, budget_ms=1200000, audit_reserve_ms=5000, fault='none', plan=None, _origin=None):
    mode = _START_MODE; start = now_ns(mode) if _origin is None else _origin
    if type(approval) is not ExecutionApproval or approval.kind != 'offline_fixture':
        raise PermissionError('execution denied: only explicit offline fixture approval exists; live is structurally unavailable')
    require(sys.platform.startswith('linux') and hasattr(os, 'waitid'), 'Linux owned process-group watchdog required')
    expected = calibration_plan(); selected = expected if plan is None else plan
    require(canonical(selected) == canonical(expected), 'exact frozen calibration plan required')
    require(type(budget_ms) is int and 0 < budget_ms <= 1200000, 'invalid lowered fixture time budget')
    require(type(audit_reserve_ms) is int and 0 < audit_reserve_ms < budget_ms, 'audit reserve required')
    require(fault in FAULTS, 'unknown fixture fault')
    deadline = start + budget_ms * 1_000_000
    out = Path(out); out.mkdir(mode=0o700)
    manifest = {'version': 'incident-calibration-controller/1', 'mode': 'offline_protocol_fixture', 'new_model_calls': 0,
                'start_ns': start, 'deadline_ns': deadline, 'work_deadline_ns': deadline-audit_reserve_ms*1_000_000,
                'sources': _sources(), 'clock_mode': mode, 'plan': selected, 'runtime': profile(), 'fault': fault,
                'accounting': 'fake_tensor_protocol_units_not_model_tokens', 'no_resume': True}
    _durable_new(out / 'manifest.json', manifest)
    require(now_ns(mode) < manifest['work_deadline_ns'], 'setup consumed execution budget')
    _supervise(out, [sys.executable, str(Path(__file__).resolve()), '_child', str(out), str(os.getpid())], deadline, mode, manifest)
    return audit_run(out)  # Read-only recovery reporting, not resumed calibration.


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if len(args) == 3 and args[0] == '_child': return _child(Path(args[1]), int(args[2]))
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('fixture'); run.add_argument('--out', type=Path, required=True)
    run.add_argument('--fault', choices=sorted(FAULTS), default='none'); run.add_argument('--budget-ms', type=int, default=1200000)
    check = sub.add_parser('audit'); check.add_argument('out', type=Path)
    options = parser.parse_args(args)
    if options.command == 'fixture':
        report = run_fixture(options.out, approval=ExecutionApproval('offline_fixture'), budget_ms=options.budget_ms,
                             audit_reserve_ms=min(5000, options.budget_ms//5), fault=options.fault, _origin=_START_NS)
    else: report = audit_run(options.out)
    print(canonical(report)); return 0 if report['complete'] else 1


if __name__ == '__main__':
    try: raise SystemExit(main())
    except Exception as error: raise SystemExit(f'Offline controller stopped: {error}')
