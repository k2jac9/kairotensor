# One-request Adaption HTTP runner

This milestone adds actual fixed-origin HTTPS transport code and a bounded
one-request process runner. **Only offline fixtures have been executed.** It
creates no credentials, reads no credentials in fixture mode, and offers no live
CLI switch. It does not establish provider availability, credit balances,
pricing, API compatibility, or model quality.

## Offline verification

```sh
python3 scripts/incident_hosted_runner.py fixture --out /tmp/incident-hosted-runner
python3 scripts/incident_hosted_runner.py audit /tmp/incident-hosted-runner
python3 -m unittest discover -s scripts -p test_incident_adaption_transport.py
python3 -m unittest discover -s scripts -p test_incident_hosted_runner.py
```

Use a fresh destination. The smoke runs a real child process with a canned HTTP
receipt, not a socket. Transport unit tests inject connection/response objects
into the same HTTP exchange logic without opening network connections. Tests
cover redirects, HTTP failures, partial responses, timeout, response-size bounds,
credential validation, deadline expiry, process crashes and hangs, duplicate
run refusal, and artifact consistency checks.

The previous `incident_hosted_controller.py` remains the separate synthetic
multi-operation accounting experiment. Its artifacts and guarantees are not
silently reused as live authority. This runner uses
`incident-hosted-single-request/1` and retains one full reservation throughout.

## Transport boundary

`incident_adaption_transport.send_https` sends exactly one POST to
`https://api.prod.adaptionlabs.ai/api/v1/responses`. It uses direct standard-library
HTTPS with default certificate and hostname verification. There are no proxy
lookups, redirects, automatic retries, connection reuse, tools, or model downloads.
The request comes from the frozen calibration renderer with tools, memory,
storage, and streaming disabled. This is a requested API configuration, not proof
that a remote provider obeyed it.

The transport allows at most 64 KiB of request bytes and preserves at most 1 MiB
of response bytes. The HTTP parser's standard header-size/count limits also
apply. Socket inactivity timeouts are set to at most ten seconds and reduced to the
remaining window where the connection socket is available. The independent
parent deadline is authoritative, including when the HTTP library transfers a
closing connection to the response object. A parent process watchdog covers DNS, connection,
slow headers/body and blocking child code. Raw allowlisted response headers and
body chunks are fsynced and atomically renamed before the next read. Incomplete
`.tmp` records remain forensic evidence but are not trusted by the audit. Exceptions record their class,
never their text or request headers. Credential bytes are not put in manifests,
process arguments, queues, or logs.

## One-shot admission and authority

The library entry point `run_live` defaults to denial. A trusted caller must
supply a `LiveApproval` obtained from an explicit external approval process:

- Exact request SHA-256 and model ID.
- An integer all-in request ceiling in micro-USD, within the run budget.
- Approval and conservative billing-bound references.
- Affirmative credential-use, data-transmission and all-billable-category scope.
- An approved maximum elapsed duration, at most twenty minutes.

Only USD all-in request ceilings are supported. This is deliberately not a token
rate calculator. The ceiling must already cover all input, output, reasoning,
fees and uncertain remote completion, using verified account-specific terms.
A reference string or boolean is a **trusted caller attestation, not independently
verified pricing evidence, a security capability or user permission**. Artifact
JSON and environment variables are never loaded as launch authorization. Do not
construct an approval to bypass missing user approval or unverified billing.

The parent validates those bindings before starting the child. SQLite commits a
single claim and full reservation before any possible request. The child checks
the original deadline, arms Linux parent-death termination, and only then reads
`ADAPTION_API_KEY` from the process environment for the fixed-origin HTTPS call.
The environment must be provisioned through an approved secure mechanism; never
paste keys into chat or put them in a command, repository, or artifact.

The runner requires Linux `CLOCK_BOOTTIME` and uses the original startup window,
including rendering and setup. It does not restart, resume or resend. The output
directory must be new. A crashed claimed run is not a reason to create another
run without resolving its possible remote charge and obtaining any required
approval.

## Accounting and failure behavior

Artifacts include an immutable manifest and request, SQLite claim, numbered raw
transport records, and the parent outcome. Offline fixtures identify their
reservation as synthetic. Authorized live runs identify it as an approved
conservative USD bound. Neither is an invoice.

The full reservation remains held even after a complete response. Provider usage
is retained separately from verified cost; `verified_provider_cost` stays null.
This milestone does not implement invoice reconciliation or release of live
holds. A transport failure, worker kill, missing receipt or partial response
therefore cannot create free budget. At most one request is attempted; no second
request can spend a released remainder in this runner.

A local timeout or killed process **does not cancel remote computation or
billing**. The amount bound must cover that possibility independently. The
watchdog bounds local child work under normal OS scheduling, not kernel stalls,
remote server execution, or final parent artifact/audit time. If durable storage
fails, some returned bytes may be lost; the committed claim still reserves the
full ceiling. Auditing rebuilds the frozen request, validates source and claim
bindings, checks ordered raw records, and reparses any retained response. These
are internal-consistency checks, not authenticated provider provenance or
protection against an attacker rewriting the entire bundle.

## Before the first live smoke

Verify the account's selected model, inference credit eligibility and effective
billing terms; establish a conservative all-category one-request ceiling; obtain
specific approval for that model, synthetic data, credential use and spending;
securely provision the key; then review and launch one request. No such live
approval, price verification, credential setup or execution is supplied by these
fixtures. The larger eight-probe calibration and fresh held-out comparison remain
separate future work. Hosted client latency cannot be substituted for a local
Qwen hardware timing profile.

The smoke records the overall startup/deadline/parent-completion window, not
individual token or per-request latency traces. It supplies no usable production
latency profile or provider cold-start measurement.
