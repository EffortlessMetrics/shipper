# Status

Status documents are the claim-maturity layer in Shipper's source-of-truth
stack. They map product claims to support tiers, proof commands, artifacts, and
owners.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| Support tiers | claim -> proof map | detailed implementation |

Support-tier documents should answer:

- Which user-facing claims are stable, advisory, experimental, or planned?
- Which command or artifact proves each stable claim?
- Which owner is responsible for keeping the claim truthful?
- Which claims are internal-only and should not be marketed as user promises?

## Neighboring Layers

- Specs define behavior that can support a claim.
- Plans and release artifacts produce the proof.
- README and product docs must not exceed the support-tier map.
- Policy ledgers explain exceptions and enforcement state behind internal
  claims.

## Rules

- A stable claim must be implemented, tested, documented, and backed by a proof
  command or artifact.
- Advisory claims may be useful but must not block release or be marketed as
  complete guarantees.
- Experimental claims are behavior that exists but is not yet a promise.
- Planned claims are roadmap intent only.
- If a README claim has no support-tier entry, either add the entry or narrow
  the claim.
