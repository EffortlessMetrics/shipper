# Failure modes and how shipper handles them

This tool treats publishing as an **irreversible, non-atomic workflow**.

## Partial publishes

If a workspace publish is interrupted after some crates have been uploaded, re-running `shipper publish` (or `shipper resume`) should skip already-published versions and continue with the remainder.

## Ambiguous timeouts

Cargo may fail locally even when the upload succeeded server-side. Shipper verifies the registry state (`crate@version exists`) before treating a step as failed.

## Rate limiting (HTTP 429)

The registry may ask clients to slow down. Shipper retries with exponential backoff + jitter and a max wall-clock limit.

## Permission mismatches

A common failure mode is having rights to publish some crates in a workspace but not all. Shipper can optionally preflight owners/permissions before publishing anything.

## CI cancellations

If your CI cancels a job mid-publish, you can re-run the job and Shipper will continue from the persisted state.
