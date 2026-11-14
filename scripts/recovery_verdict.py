"""Evidence-derived deterministic verdict baseline. No model or Store integration.

The caller supplies an admission fact; this module cannot authenticate it or check
current Store eligibility. Exactly one complete admitted report determines the
entire verdict. No reference label is accepted and no report is repaired/selected.
This is a code-based baseline/product route, not evidence of model accuracy gains.

CLI: pipe one JSON object {"reports":[...],"admitted":true} to stdin. Success emits
only the raw three-string-field JSON verdict. Refusal writes stderr and exits 2.
"""
import json
import sys

MAX_COUNT = 2**64 - 1


def _require(condition, reason):
    if not condition:
        raise ValueError(reason)


def derive_verdict(reports, admitted):
    """Return a new verdict dict or raise ValueError; inputs remain untouched."""
    _require(admitted is True, 'evidence is not admitted')
    _require(type(reports) is list and len(reports) == 1, 'exactly one complete report is required')
    report = reports[0]
    _require(type(report) is dict and set(report) == {'release', 'passed', 'total'}, 'invalid report fields')
    release, passed, total = report['release'], report['passed'], report['total']
    _require(type(release) is str and bool(release), 'release must be a nonempty string')
    # Rust/JSON string contracts use Unicode scalar values, not lone surrogates.
    try:
        release.encode('utf-8')
    except UnicodeError as error:
        raise ValueError('release contains invalid Unicode') from error
    _require(type(passed) is int and type(total) is int, 'counts must be integers, not booleans')
    _require(0 <= passed <= total <= MAX_COUNT and total > 0, 'counts outside valid unsigned-64-bit range')
    return dict(release=release, tests=f'{passed}/{total}', status='pass' if passed == total else 'fail')


def render_verdict(reports, admitted):
    """Render strict JSON without commentary, fences, or a reference-label input."""
    return json.dumps(derive_verdict(reports, admitted), ensure_ascii=True,
                      separators=(',', ':'), allow_nan=False)


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        _require(key not in result, 'duplicate JSON key')
        result[key] = value
    return result


def _reject_constant(value):
    raise ValueError('nonfinite JSON number')


def verdict_from_json(text):
    """Strictly parse the CLI envelope, rejecting duplicate keys at every level."""
    _require(type(text) is str, 'input must be JSON text')
    try:
        value = json.loads(text, object_pairs_hook=_unique_object, parse_constant=_reject_constant)
    except RecursionError as error:
        raise ValueError('JSON nesting is too deep') from error
    _require(type(value) is dict and set(value) == {'reports', 'admitted'}, 'expected only reports and admitted fields')
    return render_verdict(value['reports'], value['admitted'])


def main():
    try:
        _require(len(sys.argv) == 1, 'read one JSON envelope from stdin; no command-line arguments')
        rendered = verdict_from_json(sys.stdin.buffer.read().decode('utf-8'))
    except (ValueError, UnicodeError) as error:
        print(f'refused: {error}', file=sys.stderr)
        return 2
    print(rendered)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
