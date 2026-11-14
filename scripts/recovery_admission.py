"""Pure, conservative diagnostic retry admission. No inference, clocks or storage.

The caller must retain the ORIGINAL operation start and absolute deadline across
retries/restarts and supply now from that SAME clock domain. This helper cannot
prove clock continuity or durable deadline binding. In particular, a monotonic
clock from another host/reboot is not interchangeable without a trusted mapping.

Calibration uses the maximum observed generation-through-validation duration,
not a percentile or a guaranteed upper bound. Pass now AFTER fresh preparation,
queueing and quote acquisition; the explicit safety margin must cover remaining
unmeasured dispatch/queue overhead. Completion can still be late. Each finish component is rounded upward to an integer millisecond. Hardware/runtime
profile strings and fingerprints are caller attestations, not authenticated facts.
The function never changes a ledger, frees a sent hold, or authorizes retransmit.
"""
from dataclasses import asdict, dataclass
import hashlib
import json
import math
import re

MAX_INTEGER = 2**63 - 1


def _integer(value, minimum=0):
    return type(value) is int and minimum <= value <= MAX_INTEGER


def _number(value):
    return type(value) in (int, float) and 0 <= value <= MAX_INTEGER and math.isfinite(value)


def _identity(value):
    return type(value) is str and bool(value.strip()) and value.strip().lower() not in {'unknown', 'unspecified', 'none', 'n/a'}


@dataclass(frozen=True)
class RuntimeProfile:
    """Exact runtime identity including the full fixed prompt/format contract hash.

    prompt_sha256 may identify a fixed template with changing evidence payloads,
    not necessarily each full request; record that choice in the calibration
    protocol. max_input_tokens constrains size, not semantic workload equivalence.
    cpu_profile must fingerprint CPU model, flags and affinity; runtime_version
    and decoding must fully describe dependencies and generation settings.
    """
    model: str
    revision: str
    tokenizer_revision: str
    runtime_version: str
    dtype: str
    decoding: str
    cpu_profile: str
    threads: int
    prompt_sha256: str
    output_cap: int

    def __post_init__(self):
        for name in ('model', 'revision', 'tokenizer_revision', 'runtime_version', 'dtype', 'decoding'):
            if not _identity(getattr(self, name)):
                raise ValueError(f'known {name} required')
        for name in ('cpu_profile', 'prompt_sha256'):
            value = getattr(self, name)
            if type(value) is not str or re.fullmatch(r'[0-9a-f]{64}', value) is None:
                raise ValueError(f'{name} must be a canonical SHA-256 fingerprint')
        if not _integer(self.threads, 1) or not _integer(self.output_cap, 1):
            raise ValueError('positive integer threads and output_cap required')

    def to_dict(self):
        return asdict(self)

    @classmethod
    def from_dict(cls, value):
        if type(value) is not dict or set(value) != set(cls.__dataclass_fields__):
            raise ValueError('exact RuntimeProfile fields required')
        return cls(**value)

    @property
    def fingerprint(self):
        return hashlib.sha256(json.dumps(self.to_dict(), sort_keys=True, separators=(',', ':')).encode()).hexdigest()


@dataclass(frozen=True)
class LatencyEstimate:
    profile: RuntimeProfile
    max_duration_ms: float
    sample_count: int
    max_input_tokens: int
    estimator: str = 'calibration_max'

    def to_dict(self):
        return asdict(self)


def _valid_estimate(value):
    return (type(value) is LatencyEstimate and type(value.profile) is RuntimeProfile
            and _number(value.max_duration_ms) and value.max_duration_ms > 0
            and _integer(value.sample_count, 1) and _integer(value.max_input_tokens)
            and value.estimator == 'calibration_max')


def build_latency_estimate(profile, durations_ms, *, max_input_tokens):
    """Pool only same-profile samples supplied by the caller; never drop bad rows.

    max_input_tokens is the largest input actually represented in the calibration
    pool. It prevents extrapolation to longer requests, but smaller inputs plus a
    measured max still do not imply a statistical or hard latency guarantee.
    """
    if type(profile) is not RuntimeProfile:
        raise ValueError('known RuntimeProfile required')
    if type(durations_ms) not in (list, tuple) or not durations_ms:
        raise ValueError('nonempty measured duration list required')
    if not all(_number(duration) and duration > 0 for duration in durations_ms):
        raise ValueError('all measured durations must be finite positive numbers')
    if not _integer(max_input_tokens):
        raise ValueError('measured max_input_tokens must be a nonnegative integer')
    return LatencyEstimate(profile, max(durations_ms), len(durations_ms), max_input_tokens)


@dataclass(frozen=True)
class AdmissionDecision:
    admitted: bool
    reason: str
    estimated_finish_ms: float | None
    available_tokens: int | None
    profile: RuntimeProfile | None
    operation_start_ms: float | None
    now_ms: float | None
    deadline_ms: float | None
    safety_margin_ms: float | None
    estimate_max_duration_ms: float | None
    calibration_sample_count: int | None
    guarantee: str = 'empirical_allowance_only_not_a_completion_guarantee'

    def to_dict(self):
        return asdict(self)


def decide_admission(*, profile, estimate, operation_start_ms, now_ms, deadline_ms,
                     safety_margin_ms, input_tokens, output_cap, budget_limit,
                     spent_tokens, reserved_tokens, initial_disposition):
    """Return a fail-closed diagnostic; strict estimated finish < original deadline.

    All amounts include input plus generated tokens. Known outstanding holds stay
    reserved and reduce available capacity. Unknown accounting (including None)
    rejects admission. An uncertain initial reply cannot unlock another call.
    A real controller must still obtain the Store's fresh eligibility/reservation
    and single-use claim; this record itself is not permission to dispatch.
    """
    valid_estimate = _valid_estimate(estimate)
    available = None
    finish = None

    def result(reason, admitted=False):
        return AdmissionDecision(admitted, reason, finish, available,
            profile if type(profile) is RuntimeProfile else None,
            operation_start_ms if _number(operation_start_ms) else None,
            now_ms if _number(now_ms) else None,
            deadline_ms if _number(deadline_ms) else None,
            safety_margin_ms if _number(safety_margin_ms) else None,
            estimate.max_duration_ms if valid_estimate else None,
            estimate.sample_count if valid_estimate else None)

    if type(profile) is not RuntimeProfile:
        return result('unknown_profile')
    if estimate is None:
        return result('unknown_estimate')
    if not valid_estimate:
        return result('invalid_estimate')
    if profile != estimate.profile:
        return result('profile_mismatch')
    if not all(_number(value) for value in (operation_start_ms, now_ms, deadline_ms)):
        return result('invalid_window')
    if now_ms < operation_start_ms or deadline_ms <= operation_start_ms:
        return result('invalid_window')
    if not _number(safety_margin_ms):
        return result('invalid_margin')
    if type(initial_disposition) is not str or initial_disposition != 'invalid_output':
        return result('initial_not_retryable')
    if not _integer(input_tokens) or not _integer(output_cap, 1) or not _integer(input_tokens + output_cap):
        return result('invalid_quote')
    if not all(_integer(value) for value in (budget_limit, spent_tokens, reserved_tokens)):
        return result('invalid_budget')
    if spent_tokens + reserved_tokens > budget_limit:
        return result('invalid_budget')
    available = budget_limit - spent_tokens - reserved_tokens
    if input_tokens > estimate.max_input_tokens or output_cap != profile.output_cap:
        return result('quote_outside_calibration')
    if now_ms >= deadline_ms:
        return result('deadline_expired')
    candidate_finish = math.ceil(now_ms) + math.ceil(estimate.max_duration_ms) + math.ceil(safety_margin_ms)
    if not _number(candidate_finish):
        return result('invalid_estimate')
    finish = candidate_finish
    if input_tokens + output_cap > available:
        return result('insufficient_budget')
    if finish >= deadline_ms:
        return result('insufficient_time')
    return result('admitted', True)


# Descriptive alias for callers that name the operation rather than the decision.
admit_retry = decide_admission
