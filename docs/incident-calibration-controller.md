# Offline incident calibration controller

This is an **offline protocol and accounting test**, not a model calibration run,
production Store admission controller, throughput result, or model-quality score.
It cannot launch a model. The only child backend is a fixed Python fixture using
the existing `RequestHandler` and `PreparedBridge`, with fake tensors and synthetic
accounting units. Historical worker, Adapter, Store, and frozen corpus are unchanged.

## Commands

From the repository root:

```sh
python scripts/incident_calibration_controller.py fixture --out /tmp/incident-fixture
python scripts/incident_calibration_controller.py audit /tmp/incident-fixture
python -m unittest discover -s scripts -p test_incident_calibration_controller.py
```

The output directory must not exist. A second invocation refuses it rather than
resuming, replaying, or replacing evidence. Both commands return nonzero for an
incomplete result. There is no `--live` flag, executable/worker override, or live
approval type. The Python entry point defaults to denial; explicit
`ExecutionApproval('offline_fixture')` enables only the fixed fixture. Internal
`_child` is not an execution API: it requires a live parent, validates the same
manifest bounds, and still has no model-launch path.

## Immutable run and bounds

The controller pins the existing eight-slot calibration plan: four initial
calibration requests followed by four independently scheduled fixed-feedback
probes. These are not retries of accepted responses. Each exact request and hash,
plan, model/revision/decoding envelope, source hashes, and synthetic runtime
identity are stored in `manifest.json`. No held-out requests or labels are loaded
into the execution schedule.

Bounds are eight claims, at most 2,048 input units plus 96 output units per slot,
and 17,152 combined charged-plus-unresolved units. The fixture uses 173/211 input
units and seven output units; these are **not tokenizer measurements**. Quote
validation and cumulative accounting occur before any generate command. A known
complete previous slot is required before the next claim. No warmup or automatic
retry occurs. The plan's first cold-initial and remaining warm labels are planned
conditions, not observations of model readiness. No profile is exported.

The original absolute execution deadline starts at script startup before plan
loading/setup (or at entry to the Python API). The default is twenty minutes;
fixture tests may lower it. A separate supervisor polls the original clock at
short intervals and kills its owned child process group on expiry. A work
cutoff reserves five seconds by default for receipt/reconciliation/child audit.
A response received after that cutoff can still be recorded and charged; it
never permits another probe. The independent final read-only audit performed by
the supervisor is **post-run recovery reporting**, outside the bounded child
execution interval.

Linux `CLOCK_BOOTTIME` includes suspend. If unavailable, the clock is explicitly
labeled `fixture_monotonic`, which excludes suspend; it is not a production clock
fallback or restart-safe clock authority. No clock deadline is reset on restart,
because execution restart is prohibited. Linux parent-death signals contain both
the controller and worker if their parent disappears. The supervisor retains its
child PID unreaped while checking and killing the owned session/group, preventing
PID-reuse targeting of unrelated processes. No hostile escaped descendants,
host/kernel failure, uninterruptible I/O, exact hard-real-time kill, or cloud bill
cap is promised. A future approved real run still needs an independent host/job
bound and observed hardware/tokenizer/runtime identity.

## Durable accounting and artifacts

- `manifest.json`: exact offline run bindings and absolute window.
- `ledger.sqlite`: calibration accounting only, not Store authorization. SQLite
  rollback journal and FULL synchronization are used.
- `receipt-N.json`: independently synchronized raw receipt bytes, request hash,
  slot, and observed clock tick, written before JSON/protocol validation.
- `child-audit.json`: child audit/errors, when the child finishes normally.
- `supervisor.json`: bound deadline, exit status, termination/reaping observation.
- `controller-stderr.log`: controller diagnostics.

States are planned, reserved, claimed, received, settled, and uncertain. Immutable
request columns and append-only event records support consistency checks. The
single-use durable claim precedes transport. A failed claim rolls back and sends
nothing. A claim without valid measured usage retains the entire quote, even if
the worker was killed or likely never generated. A reserved-but-unsent slot also
remains conservatively held in this diagnostic ledger. Neither audit nor restart
releases it. Valid known usage is reconciled once; malformed output accounting,
EOF, timeout, and ambiguous transport are never charged as zero.

The audit distinguishes durable claimed slots (potential calls), settled slots,
known receipt units, ledger charges, unresolved holds, and receipts late relative
to the work cutoff. It never equates claims with proven backend executions.
Malformed usage keeps raw bytes and the hold. If SQLite receipt persistence fails,
the separate receipt can support a manual reconciliation report without replay.
If the receipt artifact fails but SQLite succeeds, known usage may still settle,
while audit completeness remains false. Simultaneous loss of both persistence
routes cannot guarantee recovery of a reply held only in memory; the last durable
claim remains unknown. A failed fsync is not advertised as a durable artifact.

## Read-only audit and trust boundary

Audit copies rollback-journal data into a temporary directory and performs any
hot-journal recovery only there. Source bytes are checked for changes during the
copy; WAL bundles are refused. The audit validates current source/runtime/plan
bindings, exact slot IDs and requests, unique coherent per-slot event chains,
quote/charge invariants, raw receipt/timestamp agreement, and aggregate limits.
Complete status additionally requires all eight settled receipts, a clean terminal
controller event, and a matching successful ended-supervisor record observed before the original deadline. `deadline_overrun` is recorded independently of whether a timeout kill was needed; an overrun retains valid accounting but cannot be complete. Event timestamps cannot exceed the sealed finish. An unsealed
snapshot may report conservative accounting, but is never a complete run. Audit
exceptions fail closed with no authoritative numeric totals.

These are internal-consistency checks, not cryptographic source authentication,
hardware attestation, tamper-proof storage, or proof against an actor able to
rewrite all artifacts. Preserve the pinned sources alongside artifacts. A changed
source revision is intentionally refused rather than silently interpreted as the
same controller. Read-only audit never sends, resumes, cancels, or reconciles by
writing to the original ledger.

## Fault coverage

Tests exercise crash after reservation, claim, and receipt; controller or worker
hang; partial/oversized IPC; worker EOF; wrong runtime/quote/usage; failed claim,
receipt, settlement, and artifact synchronization; late fallback receipts;
supervisor death; duplicate event/conflicting receipt; changed manifest and
corrupt accounting; and existing-destination refusal. Synthetic fixtures test the
protocol, not Qwen preparation measurements or language-model efficacy.
