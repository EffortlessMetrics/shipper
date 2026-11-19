# Workflow Authority Exceptions

Workflow authority is reported by:

```bash
cargo xtask check-workflow-surfaces --mode advisory
```

The default remediation is to remove or narrow the capability. An exception is valid only when a durable workflow would become incorrect without one exact capability and the remaining authority is explicitly owned, bounded, controlled, and review-dated.

The machine-readable ledger is:

```text
policy/workflow-authority-exceptions.toml
```

Validate its schema and lifecycle with:

```bash
cargo run --locked -p xtask --bin workflow-authority-exceptions
cargo test --locked -p xtask --bin workflow-authority-exceptions
```

## Exact identity

Each record binds all of the following:

```text
workflow
job
step
capability
trigger
repository
finding_repository_boundary
owner
reason
covered_by
created
review_after
```

A workflow path or capability glob is invalid. Trigger tokens use the detector's sorted, comma-separated form. The finding repository boundary records the exact detector value, while `repository` records the actual repository in which the capability is accepted.

## Lifecycle

1. Run the detector in advisory mode.
2. Remove or narrow every capability that is not required.
3. For a genuinely necessary capability, add one exact ledger record.
4. Record why removal would make the workflow incorrect and which controls bound the capability.
5. Set a near review date.
6. Keep the accepted finding visible in human and machine policy reports.
7. Delete the exception when the workflow, action, or output path no longer needs the capability.

The validator rejects:

- missing or empty required fields;
- nonexistent workflow paths;
- workflow or capability globs;
- duplicate finding identities;
- malformed, expired, or non-forward review dates;
- unsorted or duplicate trigger tokens;
- non-EffortlessMetrics repository identities;
- secret-like material in reasons or controls.

## Current ledger state

The ledger is empty. PR #287 removed the scheduled/manual Droid security scan
and deleted its job-local `contents: write` exception. Any future restoration
must re-run the detector and justify a new exact record; the historical
exception grants no current authority.

## Blocking integration

```bash
cargo xtask check-workflow-surfaces --mode blocking-allowlist
```

reconciles every detector authority finding against the ledger and fails closed on anything that is not an exact, live authorization. Both `blocking-allowlist` and `blocking-strict` fail on a reconciliation state that is not authorized; `advisory` performs the same reconciliation and reports the same states, but never fails. The ledger model is parsed once, in `xtask/src/authority_exceptions.rs`, and shared by both the validator binary and the detector — there is deliberately no second parser to drift.

Matching is on the exact six-field identity:

```text
workflow | job | step | capability | trigger | finding_repository_boundary
```

Every finding and every record lands in exactly one of five states:

| State | Meaning | Blocking |
| --- | --- | --- |
| `authorized_exceptions` | The finding matched exactly one valid, unexpired record. | No — accepted |
| `unexcepted_authority` | The finding matched no record. | Yes |
| `expired_exceptions` | The finding matched a record whose `review_after` is before today, or whose review date does not parse. | Yes |
| `drifted_exceptions` | A record matches on workflow/job/step/capability but its `trigger` or `finding_repository_boundary` differs. The finding names each drifted field with both the recorded and the detected value. | Yes |
| `unused_exceptions` | A record authorized no finding. | Yes |

A ledger that will not parse or will not validate is reported as `invalid_authority_ledger` rather than aborting the run, so the rest of the report still renders — and it blocks.

A drifted record authorizes nothing. The capability in the tree is not the capability that was reviewed, so the record is consumed by the drift finding and never counts as an authorization.

Unknown or unparsed authority shapes may not be hidden behind an ordinary accepted-capability record. The detector reports these as `unknown-permission-scalar:<value>`; a record naming that capability is rejected before matching begins, so it becomes an unused record while its intended finding stays unexcepted. Both states block. The remedy is to write permissions the detector can read, never to except the ambiguity.

Counts appear on the command's stdout summary line, in `target/policy/workflow-policy-report.json`, and in `target/policy/workflow-policy-report.md`. The raw `authority_violations` total is retained and partitioned by these buckets, so an accepted capability stays visible instead of disappearing the moment a record is written for it.

Advisory mode reports every state and blocks on none.
