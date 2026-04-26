# GitHub Issue draft — "Looking for thrashing cases (V1.7 evidence wanted)"

Copy the body below into a new GitHub issue at
`https://github.com/wei9072/aegis/issues/new`. Suggested labels:
`evidence-wanted`, `help-wanted`.

---

**Title:** Looking for thrashing cases — wanted: real LLM runs that hit 2+ consecutive REGRESSION_ROLLBACK

---

## Body

Aegis V1.6 ([sweep evidence](../v1_validation.md#v16-verification-sweep--gap-1-stalemate-detection-in-real-traffic))
verified `STALEMATE_DETECTED` firing in real LLM traffic across two
model families (5/5 ling-2.6 + 3/5 minimax-m2.5 = 8/10 runs).

**`THRASHING_DETECTED` is still mechanism-only — direct evidence
from real traffic has not been captured yet.** This issue is an open
call for that evidence.

### What "thrashing" means here

A run where the pipeline's `REGRESSION_ROLLBACK` decision pattern
fires **2 or more times in a row**. That is, the planner produces a
patch, the executor applies it, post-apply signals worsen, the
executor reverts — and then on the *next* iteration the same thing
happens again.

When that pattern is detected, Aegis emits `THRASHING_DETECTED`
([decision_pattern.py][1]) and terminates the loop with the reason
`"thrashing detected — 2 consecutive regression rollbacks; further
iterations would burn budget"`.

[1]: https://github.com/wei9072/aegis/blob/main/aegis/runtime/decision_pattern.py

### Why we don't have evidence yet

V1.6 verification used `inclusionai/ling-2.6-1t:free` and
`minimax/minimax-m2.5:free`. Both got stuck in `validation_veto`
chains before reaching `Executor.apply()` — which is the
precondition for `REGRESSION_ROLLBACK`. The state-stalemate detector
caught them; the thrashing detector never had a chance.

V1's pre-Gap-1 sweep showed `gemma × regression_rollback` did
produce 3 consecutive rollbacks, but that data is from before the
Gap 1 detector existed.

### What you can submit

Any of the following is useful evidence:

1. **A reproducible scenario** that consistently produces 2+
   consecutive `REGRESSION_ROLLBACK` events on a model you have
   access to. Ideally something you can run with
   `aegis scenario run <name>` and share the JSON snapshot.
2. **A JSON snapshot** from a real Aegis run showing
   `decision_pattern: "thrashing_detected"` in any iteration's event.
   Drop it in this issue or attach a gist.
3. **A model + scenario pairing** you suspect would thrash but
   haven't tested. We'll add it to V1.7 sweep matrix.

### How to capture a snapshot

```bash
git clone https://github.com/wei9072/aegis && cd aegis
pip install -e .  # (build instructions in README)

# Drop your scenario in tests/scenarios/<name>/ then:
PYTHONPATH=. python scripts/v1_validation.py \
    --scenarios <your-scenario> \
    --models openrouter:<your-model>:free \
    --runs 5

# Snapshots land in tests/scenarios/<name>/runs/*.json
```

If any run's `observed_patterns` array contains
`"thrashing_detected"`, that's the evidence we need. Attach the JSON
file (or paste the relevant excerpt).

### Why this is open-call instead of dev-team-fixes-it

Per
[`docs/v1_validation.md`'s framing section](../v1_validation.md#framing--what-aegis-actually-is),
Aegis is "a behavior harness for LLM-driven systems — instead of
teaching models what is good, it rejects outcomes that make the
system worse." That framing implies: validation comes from real
traffic, not from synthetic test cases.

The thrashing detector works in unit tests
([tests/test_decision_pattern.py][2]) but unit tests can't tell us
whether real-world LLM behavior exercises this code path. If your
agent pipelines through Aegis and you see thrashing fire, please
share — that's how the framework gets validated.

[2]: https://github.com/wei9072/aegis/blob/main/tests/test_decision_pattern.py

### What success looks like

Closing this issue requires at least one snapshot from a real
(non-test) Aegis run where:

```json
{
  "observed_patterns": [..., "thrashing_detected", ...]
}
```

Once captured, it lands in `docs/v1_validation.md` as the V1.7
section, mirroring how V1.6 documented STALEMATE evidence.

---

Thanks. This is the kind of validation the project can't generate by
itself.
