"""Single fixed-origin HTTPS request. No retries, redirects, SDK or tokenization.

The network function is only called by an explicitly authorized runner child.
Timeouts bound local waits, not remote computation or billing. Tests inject a
connection object into the private transport core and never open a socket.
"""
import http.client
import ssl
import time
from dataclasses import dataclass
from incident_routing import require

HOST = 'api.prod.adaptionlabs.ai'
PATH = '/api/v1/responses'
MAX_BODY = 1_048_576
MAX_REQUEST = 65_536
SAFE_HEADERS = {'content-type', 'request-id', 'x-request-id', 'retry-after'}

@dataclass(frozen=True)
class TransportResult:
    status: int | None
    body: bytes
    complete: bool
    error: str | None


def _remaining(deadline, clock):
    value = deadline - clock()
    require(0 < value <= 1200, 'transport deadline exhausted or invalid')
    return min(value, 10.0)


def _exchange(request, key, *, deadline, persist, connection_factory, clock=time.monotonic):
    """Private injection seam: persist metadata/chunks before continuing reads.

    The caller supplies only frozen bytes and an in-memory credential. Never
    write the key or exception messages to artifacts. A failure after entry is
    conservatively delivery-uncertain, including a connection failure.
    """
    require(type(request) is bytes and 0 < len(request) <= MAX_REQUEST, 'request size')
    require(type(key) is str and 0 < len(key) <= 4096 and key.isascii()
            and all(33 <= ord(c) < 127 for c in key), 'invalid credential')
    connection = None
    status = None
    body = bytearray()
    try:
        connection = connection_factory(HOST, timeout=_remaining(deadline, clock),
                                        context=ssl.create_default_context())
        # No connection reuse, redirect following or retry policy.
        connection.request('POST', PATH, body=request,
                           headers={'Authorization': 'Bearer ' + key,
                                    'Content-Type': 'application/json',
                                    'Accept': 'application/json',
                                    'Accept-Encoding': 'identity',
                                    'Connection': 'close'})
        if connection.sock is not None:
            connection.sock.settimeout(_remaining(deadline, clock))
        response = connection.getresponse()
        status = response.status
        require(type(status) is int and 100 <= status <= 599, 'HTTP status')
        headers = []
        expected_length = None
        all_headers = response.getheaders()
        require(len(all_headers) <= 100, 'header count ceiling')
        lengths = [v for k, v in all_headers if k.lower() == 'content-length']
        require(len(lengths) <= 1, 'ambiguous content length')
        if lengths:
            require(lengths[0].isascii() and lengths[0].isdigit(), 'content length')
            expected_length = int(lengths[0])
        encodings = [v.lower() for k, v in all_headers if k.lower() == 'content-encoding']
        require(all(v == 'identity' for v in encodings), 'encoded response unsupported')
        for name, value in all_headers:
            if name.lower() in SAFE_HEADERS:
                headers.append([name.lower(), value[:4096]])
        persist('headers', {'status': status, 'headers': headers})
        # A 3xx is a final response, never a new destination/request.
        while True:
            remaining = _remaining(deadline, clock)
            if connection.sock is not None:
                connection.sock.settimeout(remaining)
            # read1 returns already-received bytes without waiting for an entire
            # requested chunk; the outer process watchdog also bounds drip feeds.
            chunk = response.read1(min(16384, MAX_BODY + 1 - len(body)))
            require(type(chunk) is bytes, 'HTTP body bytes')
            if not chunk:
                require(expected_length is None or len(body) == expected_length, 'truncated HTTP body')
                persist('complete', {'status': status})
                return TransportResult(status, bytes(body), True, None)
            allowed = chunk[:MAX_BODY - len(body)]
            if allowed:
                persist('chunk', allowed)
                body.extend(allowed)
            if len(chunk) > len(allowed):
                raise ValueError('response byte ceiling')
    except Exception as error:
        # Preserve only the exception class. Network errors may embed URLs,
        # headers or credentials in their message.
        kind = type(error).__name__
        # http.client can carry bytes received before a premature EOF on the
        # exception itself; preserve that prefix without treating it as complete.
        partial = error.partial if isinstance(error, http.client.IncompleteRead) else None
        if type(partial) is bytes and partial and len(body) < MAX_BODY:
            prefix = partial[:MAX_BODY - len(body)]
            try:
                persist('chunk', prefix)
                body.extend(prefix)
            except Exception:
                pass
        try:
            persist('error', {'kind': kind})
        except Exception:
            pass
        return TransportResult(status, bytes(body), False, kind)
    finally:
        if connection is not None:
            try:
                connection.close()
            except Exception:
                pass


def send_https(request, key, *, deadline, persist):
    """Actual network code path; authorization belongs to the trusted runner."""
    return _exchange(request, key, deadline=deadline, persist=persist,
                     connection_factory=http.client.HTTPSConnection)
