"""Bounded, offline policy comparison attestations; never execution authentication."""

import hashlib
import json

MAX_BYTES = 16 * 1024 * 1024
MAX_CASES = 10_000
MAX_U64 = (1 << 64) - 1
POLICIES = ("always_stop", "affordable_retry", "evidence_gated_retry")


def require(ok, message):
    if not ok:
        raise ValueError(message)


def unique(pairs):
    out = {}
    for key, value in pairs:
        require(key not in out, "duplicate JSON key")
        out[key] = value
    return out


def loads(raw, limit=MAX_BYTES):
    require(type(raw) is bytes and len(raw) <= limit, "policy record size limit")
    depth = 0
    quoted = False
    escaped = False
    for b in raw:
        if quoted:
            if escaped:
                escaped = False
            elif b == 92:
                escaped = True
            elif b == 34:
                quoted = False
        elif b == 34:
            quoted = True
        elif b in (91, 123):
            depth += 1
            require(depth <= 64, "policy JSON depth limit")
        elif b in (93, 125):
            depth -= 1
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=unique,
            parse_constant=lambda _: require(False, "nonfinite JSON"),
        )
        bounded_tree(value)
        return value
    except (UnicodeError, RecursionError, json.JSONDecodeError) as error:
        raise ValueError("malformed policy record") from error


def bounded_tree(value):
    # Count canonical bytes without cloning containers or encoding large strings.
    size = 0

    def visit(item, depth):
        nonlocal size
        require(depth <= 64, "policy JSON depth limit")
        if type(item) is str:
            require(len(item) <= MAX_BYTES, "policy record size limit")
            size += 2
            for ch in item:
                n = ord(ch)
                require(not 0xD800 <= n <= 0xDFFF, "invalid Unicode scalar")
                if ch in ('"', "\\", "\b", "\f", "\n", "\r", "\t"):
                    size += 2
                elif n < 32:
                    size += 6
                else:
                    size += 1 if n < 128 else 2 if n < 2048 else 3 if n < 65536 else 4
                require(size <= MAX_BYTES, "policy record size limit")
        elif type(item) is dict:
            require(len(item) <= MAX_BYTES // 2, "policy record size limit")
            size += 1 + 2 * len(item) if item else 2
            for k, v in item.items():
                visit(k, depth + 1)
                visit(v, depth + 1)
        elif type(item) is list:
            require(len(item) <= MAX_BYTES // 2, "policy record size limit")
            size += 1 + len(item) if item else 2
            for v in item:
                visit(v, depth + 1)
        elif item is None:
            size += 4
        elif type(item) is bool:
            size += 4 if item else 5
        elif type(item) is int:
            size += len(str(item))
        else:
            require(False, "non-integer JSON scalar")
        require(size <= MAX_BYTES, "policy record size limit")

    visit(value, 0)


def canonical(value):
    bounded_tree(value)
    try:
        # Stream the size check before materializing the encoded representation.
        encoder = json.JSONEncoder(
            sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
        )
        size = 0
        for part in encoder.iterencode(value):
            size += len(part.encode("utf-8"))
            require(size <= MAX_BYTES, "policy record size limit")
        return encoder.encode(value).encode("utf-8")
    except (TypeError, UnicodeError, RecursionError) as error:
        raise ValueError("malformed policy record") from error


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def digest(value):
    return (
        type(value) is str
        and len(value) == 64
        and all(c in "0123456789abcdef" for c in value)
    )


def text(value):
    # Unicode White_Space, matching Rust str::trim (Python additionally strips FS/GS/RS/US).
    whitespace = "\t\n\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
    return type(value) is str and bool(value.strip(whitespace))


def uint(value):
    return type(value) is int and 0 <= value <= MAX_U64


def fields(value, names):
    require(
        type(value) is dict and set(value) == set(names.split()),
        "unknown or missing fields",
    )


def validate_policy_artifact(raw):
    a = loads(raw, 65536)
    fields(a, "schema_version policy_name purpose compatibility_contract_sha256")
    require(
        type(a["schema_version"]) is int
        and a["schema_version"] == 1
        and a["policy_name"] in POLICIES
        and text(a["purpose"])
        and digest(a["compatibility_contract_sha256"]),
        "invalid policy artifact",
    )
    return a


def byte_array(value):
    require(
        type(value) is list
        and len(value) <= MAX_BYTES
        and all(type(x) is int and 0 <= x <= 255 for x in value),
        "invalid bytes",
    )
    return bytes(value)


def validate_policy_candidate(c):
    fields(c, "version_id actor scope purpose artifact sha256")
    canonical(c)
    require(
        all(text(c[k]) for k in ("version_id", "actor", "scope", "purpose")),
        "empty candidate identity",
    )
    raw = byte_array(c["artifact"])
    a = validate_policy_artifact(raw)
    require(
        a["purpose"] == c["purpose"]
        and digest(c["sha256"])
        and sha(raw) == c["sha256"],
        "candidate identity mismatch",
    )
    return a


SELECTION_CONTRACT = b"policy-selection/1\ncomplete-paired-frozen-attestations;success=strict-increase;false-refusal,unsafe-acceptance,charged-plus-reserved,deadline-miss=nonincrease;synthetic=fixture/recovery-policy-only\n"
SELECTION_CONTRACT_SHA256 = (
    "df97ae41b2648bef15a11bf1950883597b057df1940fe43347d485fedf8590b3"
)
PROVENANCE = ("synthetic_registry_mechanics", "observed_policy_replay")
OUTCOMES = ("Accepted", "Refused", "Failed", "Unknown")
DISPOSITIONS = ("Accepted", "InvalidOutput", "Stale", "Failed", "Uncertain", "Refused")


def add(a, b):
    require(uint(a) and uint(b) and a + b <= MAX_U64, "policy accounting overflow")
    return a + b


def check_record(r):
    fields(r, "bytes sha256")
    raw = byte_array(r["bytes"])
    require(digest(r["sha256"]) and sha(raw) == r["sha256"], "record digest mismatch")
    return raw


def shape(e):
    fields(
        e,
        "evaluation_id actor scope purpose baseline_id candidate_id baseline_sha256 candidate_sha256 experiment_manifest experiment_sha256 evaluator_version evaluator_sha256 provenance baseline_rows candidate_rows",
    )
    require(
        all(
            text(e[k])
            for k in (
                "evaluation_id",
                "actor",
                "scope",
                "purpose",
                "baseline_id",
                "candidate_id",
                "evaluator_version",
            )
        ),
        "empty evaluation identity",
    )
    require(
        all(
            digest(e[k])
            for k in (
                "baseline_sha256",
                "candidate_sha256",
                "experiment_sha256",
                "evaluator_sha256",
            )
        ),
        "invalid evaluation digest",
    )
    require(e["provenance"] in PROVENANCE, "invalid provenance")
    byte_array(e["experiment_manifest"])
    for side in ("baseline_rows", "candidate_rows"):
        require(type(e[side]) is list and len(e[side]) <= MAX_CASES, "case limit")
        for r in e[side]:
            fields(
                r,
                "case_id outcome correct eligible answerable completed_ms charged reserved_bound false_refusal unsafe_acceptance deadline_miss simulated attempts",
            )
            require(
                text(r["case_id"]) and r["outcome"] in OUTCOMES,
                "invalid row identity or outcome",
            )
            require(
                all(
                    type(r[k]) is bool
                    for k in (
                        "answerable",
                        "false_refusal",
                        "unsafe_acceptance",
                        "deadline_miss",
                        "simulated",
                    )
                ),
                "invalid bool",
            )
            require(
                all(
                    r[k] is None or type(r[k]) is bool for k in ("correct", "eligible")
                ),
                "invalid nullable bool",
            )
            require(
                all(r[k] is None or uint(r[k]) for k in ("completed_ms", "charged"))
                and uint(r["reserved_bound"]),
                "invalid row count",
            )
            if r["charged"] is not None:
                add(r["charged"], r["reserved_bound"])
            require(
                type(r["attempts"]) is list and len(r["attempts"]) <= 2, "attempt limit"
            )
            for a in r["attempts"]:
                fields(
                    a,
                    "execution_id opportunity_id request_sha256 evidence_manifest_sha256 execution disposition response measured_input measured_output retained_bound start_ms completed_ms validator_sha256 scorer_sha256 label_sha256 scoring",
                )
                require(
                    text(a["execution_id"]) and text(a["opportunity_id"]),
                    "empty attempt identity",
                )
                require(
                    all(
                        digest(a[k])
                        for k in (
                            "request_sha256",
                            "evidence_manifest_sha256",
                            "validator_sha256",
                            "scorer_sha256",
                            "label_sha256",
                        )
                    ),
                    "invalid attempt digest",
                )
                require(
                    a["execution"] in ("Executed", "NotExecuted", "Unknown")
                    and a["disposition"] in DISPOSITIONS,
                    "invalid execution or disposition",
                )
                require(
                    uint(a["retained_bound"])
                    and uint(a["start_ms"])
                    and all(
                        a[k] is None or uint(a[k])
                        for k in ("measured_input", "measured_output", "completed_ms")
                    ),
                    "invalid attempt count",
                )
                if a["response"] is not None:
                    check_record(a["response"])
                if a["scoring"] is not None:
                    s = a["scoring"]
                    fields(s, "response_sha256 scorer_sha256 label_sha256 normalized")
                    require(
                        all(
                            digest(s[k])
                            for k in (
                                "response_sha256",
                                "scorer_sha256",
                                "label_sha256",
                            )
                        ),
                        "invalid scoring digest",
                    )
                    byte_array(s["normalized"])
    canonical(e)


def parse_policy_evaluation(raw):
    e = loads(raw)
    shape(e)
    return e


def evaluation_sha256(e):
    shape(e)
    return sha(b"policy-evaluation/2\n" + canonical(e))


def paired_baseline_matches(recorded, paired):
    shape(recorded)
    shape(paired)
    return all(
        recorded[k] == paired[k]
        for k in (
            "actor",
            "scope",
            "purpose",
            "baseline_id",
            "baseline_sha256",
            "experiment_manifest",
            "experiment_sha256",
            "evaluator_version",
            "evaluator_sha256",
            "provenance",
            "baseline_rows",
        )
    )


def manifest(e):
    m = loads(byte_array(e["experiment_manifest"]))
    fields(
        m,
        "format actor scope purpose provenance accounting_unit workload_sha256 prompt_sha256 runtime_sha256 model_sha256 tokenizer_sha256 validator_sha256 scorer_sha256 compatibility_contract_sha256 policies cases",
    )
    require(
        m["format"] == "policy-comparison/1"
        and type(m["cases"]) is list
        and 0 < len(m["cases"]) <= MAX_CASES
        and type(m["policies"]) is list
        and 0 < len(m["policies"]) <= 3,
        "invalid manifest format or count",
    )
    require(
        all(text(m[k]) for k in ("actor", "scope", "purpose")),
        "empty manifest identity",
    )
    require(
        m["provenance"] in PROVENANCE
        and m["accounting_unit"]
        in ("model_tokens", "fixture_bytes", "logical_support", "unknown"),
        "invalid manifest enum",
    )
    require(
        all(
            digest(m[k])
            for k in (
                "workload_sha256",
                "prompt_sha256",
                "runtime_sha256",
                "model_sha256",
                "tokenizer_sha256",
                "validator_sha256",
                "scorer_sha256",
                "compatibility_contract_sha256",
            )
        ),
        "invalid manifest digest",
    )
    names = set()
    artifacts = set()
    for p in m["policies"]:
        fields(p, "policy_name artifact")
        a = validate_policy_artifact(check_record(p["artifact"]))
        require(
            p["policy_name"] in POLICIES
            and p["policy_name"] not in names
            and p["artifact"]["sha256"] not in artifacts,
            "duplicate frozen policy",
        )
        names.add(p["policy_name"])
        artifacts.add(p["artifact"]["sha256"])
        require(
            a["policy_name"] == p["policy_name"]
            and a["purpose"] == m["purpose"]
            and a["compatibility_contract_sha256"]
            == m["compatibility_contract_sha256"],
            "frozen artifact identity mismatch",
        )
    cases = set()
    families = {}
    labels = []
    for c in m["cases"]:
        fields(
            c,
            "case_id family_id split_id initial_evidence label budget deadline_ms timing_start_ms timing_end_ms policy_opportunities",
        )
        require(
            all(text(c[k]) for k in ("case_id", "family_id", "split_id"))
            and c["case_id"] not in cases,
            "invalid or duplicate case identity",
        )
        cases.add(c["case_id"])
        require(
            all(
                uint(c[k])
                for k in ("budget", "deadline_ms", "timing_start_ms", "timing_end_ms")
            )
            and c["timing_start_ms"] <= c["deadline_ms"] <= c["timing_end_ms"],
            "invalid timing boundaries",
        )
        require(
            families.setdefault(c["family_id"], c["split_id"]) == c["split_id"],
            "family crosses splits",
        )
        check_record(c["initial_evidence"])
        labelraw = check_record(c["label"])
        l = loads(labelraw, 65536)
        fields(
            l,
            "answerable admissible_support normalized_format scorer_sha256 expected_normalized",
        )
        require(
            type(l["answerable"]) is bool
            and type(l["admissible_support"]) is bool
            and l["normalized_format"] == "opaque-bytes/1"
            and l["scorer_sha256"] == m["scorer_sha256"],
            "invalid frozen label identity",
        )
        byte_array(l["expected_normalized"])
        require(
            type(c["policy_opportunities"]) is list
            and len(c["policy_opportunities"]) == len(names),
            "missing frozen policy opportunities",
        )
        policies = set()
        for p in c["policy_opportunities"]:
            fields(p, "policy_name opportunities")
            require(
                p["policy_name"] in names and p["policy_name"] not in policies,
                "unknown or duplicate opportunity policy",
            )
            policies.add(p["policy_name"])
            require(
                type(p["opportunities"]) is list
                and len(p["opportunities"])
                == (1 if p["policy_name"] == "always_stop" else 2),
                "invalid frozen opportunity count",
            )
            ids = set()
            for i, o in enumerate(p["opportunities"]):
                fields(
                    o,
                    "opportunity_id request evidence_manifest admitted consistent input_bound output_cap start_min_ms start_max_ms",
                )
                require(
                    text(o["opportunity_id"]) and o["opportunity_id"] not in ids,
                    "duplicate opportunity identity",
                )
                ids.add(o["opportunity_id"])
                check_record(o["request"])
                check_record(o["evidence_manifest"])
                require(
                    type(o["admitted"]) is bool and type(o["consistent"]) is bool,
                    "invalid opportunity bool",
                )
                require(
                    all(
                        uint(o[k])
                        for k in (
                            "input_bound",
                            "output_cap",
                            "start_min_ms",
                            "start_max_ms",
                        )
                    ),
                    "invalid opportunity count",
                )
                add(o["input_bound"], o["output_cap"])
                require(
                    c["timing_start_ms"]
                    <= o["start_min_ms"]
                    <= o["start_max_ms"]
                    <= c["timing_end_ms"],
                    "invalid opportunity timing",
                )
                if i == 0:
                    require(
                        o["evidence_manifest"] == c["initial_evidence"],
                        "initial evidence differs",
                    )
        first = c["policy_opportunities"][0]["opportunities"][0]
        for p in c["policy_opportunities"][1:]:
            o = dict(p["opportunities"][0], opportunity_id=first["opportunity_id"])
            require(o == first, "different initial policy opportunities")
        labels.append(l)
    return m, labels


def refused(reason):
    return {"decision": "Refused", "reason": reason}


def evaluate_policy_comparison(e):
    shape(e)
    m, labels = manifest(e)
    if (
        any(e[k] != m[k] for k in ("actor", "scope", "purpose", "provenance"))
        or sha(byte_array(e["experiment_manifest"])) != e["experiment_sha256"]
        or e["evaluator_version"] != "policy-selection/1"
        or e["evaluator_sha256"] != SELECTION_CONTRACT_SHA256
        or e["baseline_id"] == e["candidate_id"]
        or m["accounting_unit"] not in ("model_tokens", "fixture_bytes")
        or (
            e["provenance"] == "synthetic_registry_mechanics"
            and e["purpose"] != "fixture/recovery-policy"
        )
    ):
        return refused("Incomparable")
    selected = {p["artifact"]["sha256"]: p["policy_name"] for p in m["policies"]}
    if any(e[k] not in selected for k in ("baseline_sha256", "candidate_sha256")):
        return refused("Incomparable")
    executions = {}
    b = rows(m, labels, e["baseline_rows"], selected[e["baseline_sha256"]], executions)
    c = rows(
        m, labels, e["candidate_rows"], selected[e["candidate_sha256"]], executions
    )
    if type(b) is str:
        return refused(b)
    if type(c) is str:
        return refused(c)
    if c["success"] <= b["success"]:
        return refused("NoImprovement")
    for key, reason in [
        ("false_refusal", "IncreasedFalseRefusal"),
        ("unsafe_acceptance", "IncreasedUnsafeAcceptance"),
        ("cost", "IncreasedCost"),
        ("deadline_misses", "IncreasedDeadlineMisses"),
    ]:
        if c[key] > b[key]:
            return refused(reason)
    return {
        "decision": "Eligible",
        "validated_counters": {"baseline": b, "candidate": c},
    }


def rows(m, labels, records, policy, executions):
    issue = "Incomplete" if len(records) != len(m["cases"]) else None
    counters = {
        "cases": 0,
        "success": 0,
        "false_refusal": 0,
        "unsafe_acceptance": 0,
        "cost": 0,
        "deadline_misses": 0,
    }
    for r, c, l in zip(records, m["cases"], labels):
        if r["case_id"] != c["case_id"]:
            issue = "Incomparable"
            continue
        require(r["answerable"] == l["answerable"], "fabricated answerability")
        require(
            r["false_refusal"]
            == (
                r["outcome"] == "Refused"
                and l["answerable"]
                and l["admissible_support"]
            ),
            "fabricated false refusal",
        )
        if r["outcome"] != "Accepted":
            require(not r["unsafe_acceptance"], "fabricated unsafe acceptance")
        if r["correct"] is not None and r["eligible"] is not None:
            require(
                r["unsafe_acceptance"]
                == (
                    r["outcome"] == "Accepted"
                    and (not r["correct"] or not r["eligible"])
                ),
                "fabricated unsafe acceptance",
            )
        if r["completed_ms"] is not None:
            require(
                c["timing_start_ms"] <= r["completed_ms"] <= c["timing_end_ms"]
                and r["deadline_miss"] == (r["completed_ms"] > c["deadline_ms"]),
                "fabricated deadline miss",
            )
        if r["charged"] is not None:
            require(
                add(r["charged"], r["reserved_bound"]) <= c["budget"],
                "row exceeds frozen budget",
            )
        if m["provenance"] == "synthetic_registry_mechanics":
            require(
                r["simulated"] and not r["attempts"],
                "synthetic rows require simulated counters only",
            )
            if r["eligible"] is not None:
                require(
                    r["eligible"] == l["admissible_support"],
                    "fabricated synthetic eligibility",
                )
        else:
            require(not r["simulated"], "observed row cannot simulate counters")
            reason = observed(m, c, l, r, policy, executions)
            if reason:
                issue = issue or reason
                continue
        if (
            any(
                r[k] is None for k in ("correct", "eligible", "completed_ms", "charged")
            )
            or r["outcome"] == "Unknown"
        ):
            issue = issue or "Incomplete"
            continue
        require(
            c["timing_start_ms"] <= r["completed_ms"] <= c["timing_end_ms"],
            "row outside timing window",
        )
        require(
            r["false_refusal"]
            == (
                r["outcome"] == "Refused"
                and l["answerable"]
                and l["admissible_support"]
            ),
            "fabricated false refusal",
        )
        require(
            r["unsafe_acceptance"]
            == (r["outcome"] == "Accepted" and (not r["correct"] or not r["eligible"])),
            "fabricated unsafe acceptance",
        )
        require(
            r["deadline_miss"] == (r["completed_ms"] > c["deadline_ms"]),
            "fabricated deadline miss",
        )
        cost = add(r["charged"], r["reserved_bound"])
        require(cost <= c["budget"], "row exceeds frozen budget")
        values = {
            "cases": 1,
            "success": int(
                r["outcome"] == "Accepted"
                and r["correct"]
                and r["eligible"]
                and not r["deadline_miss"]
            ),
            "false_refusal": int(r["false_refusal"]),
            "unsafe_acceptance": int(r["unsafe_acceptance"]),
            "cost": cost,
            "deadline_misses": int(r["deadline_miss"]),
        }
        for k, v in values.items():
            counters[k] = add(counters[k], v)
    return issue or counters


def observed(m, c, l, r, policy, executions):
    ops = next(
        p["opportunities"]
        for p in c["policy_opportunities"]
        if p["policy_name"] == policy
    )
    if not r["attempts"]:
        return "Incomplete"
    require(len(r["attempts"]) <= len(ops), "too many observed attempts")
    charged = 0
    previous_end = c["timing_start_ms"]
    result = ("Refused", False, l["admissible_support"], previous_end)
    ids = set()
    for i, a in enumerate(r["attempts"]):
        o = ops[i]
        if (
            a["opportunity_id"] != o["opportunity_id"]
            or a["request_sha256"] != o["request"]["sha256"]
            or a["evidence_manifest_sha256"] != o["evidence_manifest"]["sha256"]
            or a["validator_sha256"] != m["validator_sha256"]
            or a["scorer_sha256"] != m["scorer_sha256"]
            or a["label_sha256"] != c["label"]["sha256"]
        ):
            return "Incomparable"
        require(a["execution_id"] not in ids, "reused execution in one policy path")
        ids.add(a["execution_id"])
        if i > 0:
            require(
                r["attempts"][i - 1]["execution"] == "Executed"
                and r["attempts"][i - 1]["disposition"] == "InvalidOutput",
                "only settled invalid output unlocks retry",
            )
        require(
            a["start_ms"] >= previous_end
            and o["start_min_ms"] <= a["start_ms"] <= o["start_max_ms"],
            "attempt outside timing window",
        )
        quote = add(o["input_bound"], o["output_cap"])
        require(charged <= c["budget"], "budget exceeded")
        gate = (
            o["admitted"]
            and not (i > 0 and policy == "evidence_gated_retry" and not o["consistent"])
            and a["start_ms"] < c["deadline_ms"]
            and quote <= c["budget"] - charged
        )
        if a["execution"] == "Unknown":
            require(
                a["measured_input"] is None
                and a["measured_output"] is None
                and a["response"] is None
                and a["scoring"] is None
                and a["retained_bound"] == quote
                and a["disposition"] == "Uncertain"
                and gate
                and i + 1 == len(r["attempts"]),
                "invalid unknown execution",
            )
            require(
                r["charged"] is None
                and r["reserved_bound"] == quote
                and r["outcome"] == "Unknown",
                "unknown row accounting",
            )
            return "Incomplete"
        if a["execution"] == "NotExecuted":
            require(
                not gate
                and a["disposition"] == "Refused"
                and a["measured_input"] == 0
                and a["measured_output"] == 0
                and a["retained_bound"] == 0
                and a["response"] is None
                and a["scoring"] is None
                and a["completed_ms"] == a["start_ms"]
                and i + 1 == len(r["attempts"]),
                "invalid nonexecution attestation",
            )
            result = (
                "Refused",
                False,
                o["admitted"] and l["admissible_support"],
                a["start_ms"],
            )
        else:
            require(gate, "execution violates frozen admission")
            if a["measured_input"] is not None:
                require(a["measured_input"] <= o["input_bound"], "input exceeds quote")
            if a["measured_output"] is not None:
                require(a["measured_output"] <= o["output_cap"], "output exceeds cap")
            if a["measured_input"] is None or a["measured_output"] is None:
                require(
                    a["retained_bound"] == quote
                    and r["charged"] is None
                    and r["reserved_bound"] == quote
                    and r["outcome"] == "Unknown",
                    "unknown usage must retain full bound",
                )
                require(
                    i + 1 == len(r["attempts"]), "unknown usage cannot unlock retry"
                )
                return "Incomplete"
            require(
                a["retained_bound"] == 0 and a["disposition"] != "Uncertain",
                "settled usage has retained hold or uncertain disposition",
            )
            charged = add(charged, add(a["measured_input"], a["measured_output"]))
            end = a["completed_ms"]
            if end is None:
                return "Incomplete"
            require(
                a["start_ms"] <= end <= c["timing_end_ms"], "invalid completion time"
            )
            previous_end = end
            if a["response"] is not None:
                s = a["scoring"]
                if s is None:
                    return "Incomplete"
                if (
                    s["response_sha256"] != a["response"]["sha256"]
                    or s["scorer_sha256"] != m["scorer_sha256"]
                    or s["label_sha256"] != c["label"]["sha256"]
                ):
                    return "Incomparable"
                correct = s["normalized"] == l["expected_normalized"]
            else:
                require(a["scoring"] is None, "scoring without response")
                if a["disposition"] != "Failed":
                    return "Incomplete"
                correct = False
            outcome = (
                "Accepted"
                if a["disposition"] == "Accepted"
                else ("Failed" if a["disposition"] == "Failed" else "Refused")
            )
            result = (
                outcome,
                correct,
                o["admitted"]
                and l["admissible_support"]
                and a["disposition"] != "Stale",
                end,
            )
            if (
                a["disposition"] == "InvalidOutput"
                and i + 1 < len(ops)
                and i + 1 == len(r["attempts"])
            ):
                return "Incomplete"
            if a["disposition"] != "InvalidOutput":
                require(
                    i + 1 == len(r["attempts"]),
                    "terminal disposition cannot unlock retry",
                )
        observation = (c["case_id"], dict(a, opportunity_id=""))
        if (
            a["execution_id"] in executions
            and executions[a["execution_id"]] != observation
        ):
            return "Incomparable"
        executions[a["execution_id"]] = observation
    require(
        (r["outcome"] == "Unknown" or r["outcome"] == result[0])
        and all(
            r[k] is None or r[k] == v
            for k, v in zip(
                ("correct", "eligible", "completed_ms", "charged"),
                (*result[1:], charged),
            )
        )
        and r["reserved_bound"] == 0,
        "fabricated observed row totals",
    )
    if r["outcome"] == "Unknown" or any(
        r[k] is None for k in ("correct", "eligible", "completed_ms", "charged")
    ):
        return "Incomplete"
    return None


def convert_legacy(document, comparison=None):
    """No missing facts are inferred. A supplied complete attestation must bind this input.

    workload_sha256 binds the compact sorted-key UTF-8 encoding of the archived
    document. Request strings, scheduling, quotes and legacy measured totals are
    checked in addition. Evidence contents, labels and normalization remain the
    supplying producer's attestations, not facts created by this converter.
    """
    from recovery_policy_eval import evaluate

    legacy = evaluate(document)
    source_sha = sha(canonical(document))
    result = {
        "format": "policy-evaluation-conversion/2",
        "status": "Incomplete",
        "legacy_input_sha256": source_sha,
        "reason": "A matching complete frozen comparison attestation is required; legacy fields do not establish answerability, eligibility, units or observed references.",
    }
    if comparison is None:
        return result
    shape(comparison)
    m, _ = manifest(comparison)
    expected_provenance = (
        "synthetic_registry_mechanics"
        if document["mode"] == "synthetic_policy_fixture"
        else "observed_policy_replay"
    )
    if (
        m["workload_sha256"] != source_sha
        or m["accounting_unit"] != "model_tokens"
        or m["provenance"] != expected_provenance
    ):
        return result
    if len(m["cases"]) != len(document["cases"]):
        return result
    for c, old in zip(m["cases"], document["cases"]):
        if any(
            c[k] != old[v]
            for k, v in [
                ("case_id", "case_id"),
                ("family_id", "family"),
                ("split_id", "split"),
                ("budget", "budget_tokens"),
                ("deadline_ms", "deadline_ms"),
            ]
        ):
            return result
        if c["timing_start_ms"] != 0:
            return result
        for p in c["policy_opportunities"]:
            for i, o in enumerate(p["opportunities"]):
                cell = old["retries"].get(p["policy_name"]) if i else None
                if i and cell is None:
                    return result
                ctx = cell["context"] if i else old["initial_context"]
                if byte_array(o["request"]["bytes"]) != ctx["request"].encode("utf-8"):
                    return result
                if any(
                    o[k] != ctx[v]
                    for k, v in [
                        ("admitted", "admitted"),
                        ("consistent", "consistent"),
                        ("input_bound", "input_tokens"),
                        ("output_cap", "output_cap"),
                    ]
                ):
                    return result
    policies = {p["artifact"]["sha256"]: p["policy_name"] for p in m["policies"]}
    for side, identity in [
        ("baseline_rows", "baseline_sha256"),
        ("candidate_rows", "candidate_sha256"),
    ]:
        policy = policies.get(comparison[identity])
        oldrows = [r for r in legacy["cases"] if r["policy"] == policy]
        if len(oldrows) != len(comparison[side]):
            return result
        for row, old in zip(comparison[side], oldrows):
            if old["reason"] == "uncertain":
                return result
            if any(
                row[k] != old[v]
                for k, v in [
                    ("case_id", "case_id"),
                    ("correct", "correct"),
                    ("completed_ms", "elapsed_ms"),
                    ("charged", "charged_tokens"),
                    ("reserved_bound", "reserved_tokens"),
                ]
            ):
                return result
            if row["outcome"] != (
                "Refused"
                if old["abstained"] or old["reason"] == "stale"
                else "Accepted"
            ):
                return result
            success = (
                row["outcome"] == "Accepted"
                and row["correct"] is True
                and row["eligible"] is True
                and row["completed_ms"]
                <= next(
                    c["deadline_ms"]
                    for c in m["cases"]
                    if c["case_id"] == row["case_id"]
                )
            )
            if success != old["success"]:
                return result
    selection = evaluate_policy_comparison(comparison)
    if selection == refused("Incomplete"):
        return result
    return {
        "format": "policy-evaluation-conversion/2",
        "status": "Complete",
        "legacy_input_sha256": source_sha,
        "evaluation": comparison,
        "selection": selection,
    }
