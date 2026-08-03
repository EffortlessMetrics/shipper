# Issue #153 Acceptance Ledger

This ledger records the current proof boundary for sequential/parallel package
execution. A row is complete only when the cited test runs both schedulers and
compares the semantic evidence listed in the issue. Existing focused tests are
listed so the conformance corpus does not duplicate coverage without adding a
missing assertion.

| Scenario | Existing focused proof | Both modes? | Missing assertion or next work |
| --- | --- | --- | --- |
| Already published | `mode_parity_corpus_sequential_matches_parallel` (`already_published_in_state`); `run_publish_already_published_packages_skipped_in_state` | Yes for corpus | Compare receipt evidence and event order explicitly in the corpus. |
| Clean publish | `mode_parity_corpus_sequential_matches_parallel` (`clean_publish`) | Yes | Compare receipt execution evidence and cargo invocation count. |
| Retryable failure and exhaustion | `test_run_publish_mode_retry_parity_for_multi_level_plan`; `publish_package_retry_exhaustion_preserves_retryable_class` | Focused mode parity exists | Add the scenario to the shared corpus with attempt history, backoff, and final error-class assertions. |
| Permanent failure | `mode_parity_corpus_sequential_matches_parallel` (`permanent_failure`); `test_publish_package_handles_permanent_failure` | Yes for corpus | Compare receipt reason and terminal event sequence. |
| Ambiguous result -> visible | `reconcile_bdd_ambiguous_resolves_to_published` | No shared corpus proof | Add sequential/parallel comparison without normalizing semantic evidence. |
| Ambiguous result -> not published and safe retry | `reconcile_bdd_ambiguous_resolves_to_not_published_then_retries` | No shared corpus proof | Add both-mode cargo count, retry, and rebuild assertions. |
| Ambiguous result -> StillUnknown | `mode_parity_corpus_sequential_matches_parallel` (`still_unknown_resume`); `reconcile_bdd_ambiguous_resolves_to_still_unknown` | Yes for corpus | Compare reconciliation receipt/report evidence. |
| Upload -> readiness timeout -> later visibility | `mode_parity_corpus_sequential_matches_parallel` (`readiness_timeout_then_visible`) | Yes | Compare readiness evidence delay and event sequence in the full receipt. |
| Interruption and resume | `rebuild_interrupt_resumes_from_uploaded_checkpoint`; `test_run_publish_mode_parity_from_uploaded_checkpoint` | Partial | Add end-to-end sequential/parallel resume receipt comparison. |
| `--resume-from` skip | `test_resume_from_skip_mode_parity_preserves_durable_evidence`; `run_publish_resume_from_skips_before_and_warns`; `test_resume_from_skips_earlier_levels` | Yes | Parallel fabricated receipt path removed; events, state, receipts, and cargo count are compared. Extend to multi-level and non-terminal pre-resume packages. |
| Partial multi-package failure | `sm_multi_package_partial_progress`; `test_partial_success_within_level` | Partial | Add one shared scenario with synchronized caller-visible state and receipt comparison. |
| Parallel worker/join failure | No focused proof | No sequential analogue | Model as explicit parallel-only run error with synchronized state and replayed reporter output. |
| Webhook delivery failure | `test_webhook_failure_is_non_blocking_and_mode_parity_holds`; `run_publish_sequential_webhooks_send_started_and_completed_once`; `run_publish_parallel_webhooks_send_started_and_completed_once` | Yes | Preparation-failure notification guard is covered by `run_publish_errors_on_invalid_resume_from_target`; later corpus work should compare package/run counts with the full receipt. |
| Event-write failure | `mode_parity_corpus_sequential_matches_parallel` (`event_write_failure`) | Yes | Add state/rebuild assertions for the failure boundary. |
| State-write failure | `sm_sequential_scheduler_restores_event_log_on_skip_write_failure` | No | Add the parallel counterpart and compare caller-visible error/state. |
| Event-to-state rebuild equivalence | `assert_mode_parity_rebuild` in the shared corpus; `state::rebuild` tests | Partial | Apply to every applicable scenario and compare proved fields, not timestamps or paths. |

## Selected slice

This PR closes the `--resume-from` production divergence and centralizes the
run-start notification after successful preparation. It does not claim the remaining
conformance rows are complete; those rows remain follow-up work for the final
`#153` corpus PR.

Proof for this slice:

```text
cargo fmt --all -- --check
cargo test -p shipper-core test_resume_from_skips_earlier_levels --locked
cargo test -p shipper-core test_webhook_failure_is_non_blocking_and_mode_parity_holds --locked
cargo clippy -p shipper-core --all-targets --all-features --locked -- -D warnings
git diff --check
```
