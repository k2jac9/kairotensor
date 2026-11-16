"""Decision v1 byte integrity reader, not a Rust ledger/contract/access validator.

Public helpers return recorded facts only. Callers deciding contract applicability
must validate it independently, including roles, manifest and runtime bindings.
"""
import hashlib
import json
from pathlib import Path
import re

MAX_BUNDLE_BYTES = 16 * 1024 * 1024
MAX_PAYLOAD_BYTES = 4 * 1024 * 1024
MAX_JSON_DEPTH = 64
DIGEST = re.compile(r'[0-9a-f]{64}\Z')
HEX = re.compile(r'[0-9a-f]*\Z')
TOP_FIELDS = {'schema_version', 'root_attempt', 'actor', 'scope', 'export_purpose',
              'snapshot', 'attempts', 'evidence', 'integrity'}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def canonical_bytes(value):
    """Independent canonical v1 encoding. Not an authorization operation."""
    return json.dumps(value, ensure_ascii=False, sort_keys=True,
                      separators=(',', ':'), allow_nan=False).encode('utf-8')


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON field')
        result[key] = value
    return result


def reject_number(_):
    raise ValueError('non-integer JSON number')


def bounded_depth(raw):
    depth, quoted, escaped = 0, False, False
    for byte in raw:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in (91, 123):
            depth += 1
            require(depth <= MAX_JSON_DEPTH, 'JSON depth limit exceeded')
        elif byte in (93, 125):
            depth -= 1


def decode_envelope(envelope):
    """Check bounds, exact lowercase hex, byte length and digest; preserve bytes."""
    require(type(envelope) is dict and set(envelope) == {'hex', 'byte_len', 'sha256'}, 'invalid byte envelope')
    length, encoded, digest = envelope['byte_len'], envelope['hex'], envelope['sha256']
    require(type(length) is int and 0 <= length <= MAX_PAYLOAD_BYTES, 'byte length limit or type')
    require(type(encoded) is str and len(encoded) <= 2 * MAX_PAYLOAD_BYTES
            and len(encoded) == 2 * length and HEX.fullmatch(encoded), 'invalid lowercase hex')
    require(type(digest) is str and DIGEST.fullmatch(digest), 'invalid byte digest')
    raw = bytes.fromhex(encoded)
    require(hashlib.sha256(raw).hexdigest() == digest, 'byte digest mismatch')
    return raw


def check_envelopes(value):
    # Every byte envelope in v1 has these exact three keys. Partial envelopes
    # also refuse; other record/ledger semantics remain the Rust validator's job.
    if type(value) is dict:
        if ('hex' in value or 'byte_len' in value
                or ('sha256' in value and 'version' not in value)):
            decode_envelope(value)
        else:
            for child in value.values():
                check_envelopes(child)
    elif type(value) is list:
        for child in value:
            check_envelopes(child)


def parse_bundle(raw):
    """Read bounded complete canonical-payload JSON; refuse export wrappers.

    Checks duplicate keys, depth, top-level v1 shape, envelope integrity and the
    outer canonical digest. Does not implement Rust's full wire or ledger schema.
    """
    require(type(raw) is bytes and len(raw) <= MAX_BUNDLE_BYTES, 'transport limit exceeded')
    bounded_depth(raw)
    try:
        value = json.loads(raw.decode('utf-8'), object_pairs_hook=unique_object,
                           parse_float=reject_number, parse_constant=reject_number)
        require(type(value) is dict, 'expected complete decision object')
        if value.get('status') == 'redacted':
            raise ValueError('redacted export unavailable; not replayable or scored')
        require(set(value) == TOP_FIELDS and type(value['schema_version']) is int
                and value['schema_version'] == 1, 'unsupported decision v1 shape or version')
        require(type(value['attempts']) is list and len(value['attempts']) <= 128
                and all(type(a) is dict for a in value['attempts'])
                and type(value['evidence']) is list, 'invalid record arrays')
        integrity = value['integrity']
        require(type(integrity) is dict and set(integrity) == {'version', 'sha256'}
                and integrity['version'] == 'decision-bundle-integrity/1'
                and type(integrity['sha256']) is str and DIGEST.fullmatch(integrity['sha256']), 'invalid integrity manifest')
        # Work on the bounded parsed object and restore the original digest even
        # on failure; callers always receive the unmodified recorded object.
        digest = integrity.pop('sha256')
        try:
            actual = hashlib.sha256(canonical_bytes(value)).hexdigest()
        finally:
            integrity['sha256'] = digest
        require(actual == digest, 'outer integrity digest mismatch')
        check_envelopes(value)
        return value
    except (UnicodeError, RecursionError, OverflowError) as error:
        raise ValueError('invalid decision encoding') from error


def read_bundle(path):
    """Allocate at most the transport bound plus one before JSON parsing."""
    with Path(path).open('rb') as source:
        raw = source.read(MAX_BUNDLE_BYTES + 1)
    return parse_bundle(raw)


def inspect_bundle(bundle):
    """Summarize integrity-checked records without asserting applicability."""
    # Also enforce our checks for public callers passing a constructed object.
    bundle = parse_bundle(canonical_bytes(bundle))
    attempts = bundle['attempts']
    validators = set()
    for attempt in attempts:
        binding = attempt.get('validator')
        if type(binding) is dict and type(binding.get('version')) is str:
            validators.add(binding['version'])
    return dict(status='integrity_checked', schema_version=1,
                source_bundle_sha256=hashlib.sha256(canonical_bytes(bundle)).hexdigest(),
                attempt_records=len(attempts), evidence_records=len(bundle['evidence']),
                declared_validators=sorted(validators),
                rust_ledger_validation='not_performed', validator_execution='not_performed',
                historical_timing='not_reproduced', current_access='not_authenticated',
                limitations='Hashes establish internal byte consistency only; recorded identities, roles, contracts, availability and acceptance require independent validation. No policy or cost score is produced.')


def audit_file(path):
    """Versioned audit CLI helper, including a distinct transport identity."""
    with Path(path).open('rb') as source:
        raw = source.read(MAX_BUNDLE_BYTES + 1)
    result = inspect_bundle(parse_bundle(raw))
    result['transport_file_sha256'] = hashlib.sha256(raw).hexdigest()
    return result
