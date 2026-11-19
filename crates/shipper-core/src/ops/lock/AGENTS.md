# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

# Module: `crate::ops::lock`

**Layer:** ops (layer 1, bottom).
**Single responsibility:** Exclusive ownership for cooperating Shipper processes
using one persistent OS-lock sidecar per existing workspace/state-directory key.
The separate JSON record supplies advisory holder and process-identity evidence.

## Public and internal APIs

The public path is `shipper_core::lock`; core callers use `crate::lock`. The
install-facing `shipper` facade does not re-export this module.

- `LOCK_FILE`, `LockInfo`, and `lock_path` retain their existing public shapes.
- `LockFile::acquire` refuses any retained metadata. `acquire_with_timeout`
  permits conservative recovery. `acquire_with_timeout_and_force` adds an
  explicit operator override for unknown metadata, subject to the rules below.
- `release(&self)`, `set_plan_id(&self)`, and Drop serialize on the handle's
  mutex. A released handle cannot modify a successor's record or release its
  guard. The optional JSON ownership protocol and nonce are additive fields;
  `LockInfo` remains readable by existing consumers.
- `is_locked` reports advisory JSON presence only. It is not an ownership test
  and a pre-check followed by acquisition is never an atomic admission rule.
- Internal `LockReservation::acquire(state_dir, workspace_root, timeout, force)`
  acquires the OS guard and captures `prior_contents()` without modifying JSON.
  An unclaimed reservation's Drop preserves every predecessor byte. Orchestration
  can reload retained artifacts under ownership, apply its admission policy,
  then `claim(plan_id)` on the same handle.
- `LockFile::read_owned_record` validates an active handle and its nonce without
  attempting to relock that handle. Runtime liveness and upload admission remain
  higher-layer decisions; this module does not import or decide them.

## Ownership and recovery rules

1. Every acquisition opens a fresh read/write handle to `<lock_path>.guard` and
   calls safe `std::fs::File::try_lock`. Contention, unsupported locking, and all
   other OS errors fail closed, including for force and clean.
2. The sidecar is persistent. Never unlink, rename, replace, or truncate it during
   acquisition, release, Drop, force, or cleanup. Removing its inode can create
   two independently locked files at the same path. Do not clone a held handle
   or call `try_lock` again on one that is already held.
3. A same-host record with the recognized ownership protocol may be recovered
   after acquiring the OS guard. Its age and still-running PID do not imply
   continuing ownership of that released handle.
4. Automatic legacy recovery requires the same hostname and known process scope,
   a positive observation that the recorded process is absent, and a nonnegative
   age strictly greater than the configured timeout. An exact boundary or future
   timestamp is not stale. Missing process identity, mismatched scope, a failed
   probe, corrupt JSON, unknown protocol, and foreign-host records stay unknown.
   Malformed or duplicate additive ownership fields do not erase readable base
   process identity; a positively live legacy holder still refuses force.
5. Explicit force may override unknown metadata after winning the OS guard. The
   operator assumes a noncooperating legacy or foreign publisher is quiescent.
   Force never steals an actually held guard or a positively observed live
   legacy holder. It does not override the engine's retained-state admission.
6. JSON updates use an exclusive temporary file and rename while ownership is
   held. Release removes JSON only when its nonce still matches. Missing,
   corrupt, or replaced metadata is preserved. The private, uncloneable guard
   explicitly unlocks before close, so a forked pre-exec child cannot prolong
   released ownership through a temporary duplicate descriptor. Explicit release
   reports unlock I/O failure; Drop is best effort, with close as fallback. The
   lifecycle mutex remains held throughout cleanup and unlock. A released handle
   cannot touch a new owner.

## Compatibility and proof limits

`lock_path` retains the existing `DefaultHasher` workspace key. Aliases,
collisions, and cross-toolchain hash stability are unchanged limitations.
The guarantee covers cooperating processes using the same key on a filesystem
with supported OS locking. It is not a distributed-lock, network-filesystem,
hostile external edit, old-binary, or power-loss durability guarantee. Linux
process-identity observations are a separate conservative legacy-recovery aid;
on unsupported platforms they remain unknown.

Keep deterministic thread and process rendezvous tests: retain winning handles
until all contenders finish, require exactly one owner, cover abandonment, and
exercise stale A after release against successor B and contender C. A test-only
duplicate descriptor models pre-exec inheritance and proves that its later close
cannot release a successor. Test refused
cleanup and engine entry paths with byte-for-byte artifact preservation and no
Cargo process. Native Windows and other filesystem behavior requires its own
execution evidence; Linux proof cannot establish it.

## Architectural constraints

Layer-1 synchronous I/O only. Do not import `engine`, `plan`, `state`, or `runtime`.
No unsafe code or new crate is needed. The implementation uses standard file
locking plus existing `anyhow`, `serde`, `serde_json`, `chrono`, `gethostname`,
and `rand` dependencies. Parent-directory sync remains best effort.
