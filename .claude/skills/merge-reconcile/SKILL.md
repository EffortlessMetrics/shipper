---
name: merge-reconcile
description: Merge an integration-ready shipper-swarm PR at the reviewed head and reconcile post-merge state. Use only after review-pr is REVIEW_CURRENT and verify-live-ci is INTEGRATION_READY.
---

# Merge and reconcile

## Preconditions

Require:

```text
substantive result = REVIEW_CURRENT
integration posture = INTEGRATION_READY
expected reviewed head = current PR head
merge authorized by the user or standing workflow
```

Re-read the PR immediately before mutation. Abort if the head moved or the integration posture is no longer current.

## Merge

- Squash-merge normal development PRs into `shipper-swarm/main`.
- Do not squash or rebase a history-preserving source/swarm synchronization PR.
- Preserve the PR’s actual claim and issue references in the squash title/body.
- Do not enable tag, publish, signing, deployment, credential, GitHub Release, or release-authority operations.

## Post-merge reconciliation

After merge, reload and record:

- merge/squash commit and resulting `main` head;
- merged-tree health and any required post-merge run;
- controlling issue state and remaining acceptance predicates;
- dependent/stacked PR bases and whether their earlier review is stale;
- branch/worktree cleanup posture;
- retained review, CI, and proof evidence;
- explicit publication authorization, which remains `NO` unless the release-authority process separately records otherwise.

A successful GitHub merge response is not the end of the task. If merged `main` is red or the issue remains materially incomplete, report and own the exact next repair rather than declaring completion.
