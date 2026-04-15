# Decrating Plan: Microcrate Consolidation & Architectural Modularization

**Status:** Planning complete, execution pending
**Target:** v0.3.0 release
**Branch:** TBD (will branch from `main` after current `fix/main-ci-and-audit` merges)

---

## 1. Executive Summary

The Shipper workspace currently has **30 published-eligible crates**, with most of them being internal orchestration seams that were extracted as separate packages. This creates an unsustainable public surface for crates.io: 30 semver promises, 30 docs.rs pages, 30 release-sequence steps per version, and a high risk of partial-publish failures.

The repo also carries a **dual-implementation pattern**: many subsystems exist twice — once as `crates/shipper/src/<name>.rs` (in-tree, used when `micro-<name>` feature is OFF) and once as `crates/shipper/src/<name>_micro.rs` (a shim that delegates to the standalone `shipper-<name>` microcrate, used when ON). The CLI defaults to `micro-all`, so the production code path is the shim+microcrate path. This dual implementation is architectural rot independent of the publish question.

**Target state:**

- **13 published crates** (down from 30)
- **Zero dual implementations** — one canonical source per concept
- **Strong architectural separation preserved** via folder-based module structure inside `shipper`, with one folder per absorbed microcrate
- **One-direction layered architecture** inside `shipper`: `engine → plan → state → runtime → ops`
- **Per-folder `CLAUDE.md`** files for module-scoped agent context
- **No `micro-*` feature flags** anywhere

The substitution is **SRP-by-microcrate → SRP-by-module**, one-for-one. No responsibilities are merged or diluted; the boundary just moves from `Cargo.toml` to `mod.rs` + `pub(crate)`.

---

## 2. Why This Direction

### 2.1 The current state is already halfway here

`crates/shipper/src/lib.rs` already conditionally selects between in-tree modules and `*_micro.rs` shims via `#[cfg(feature = "micro-*")]`. The repo has been telegraphing "the microcrate split was over-aggressive" for a while.

### 2.2 Cargo enforces the choice

A published crate cannot keep `path`-only dependencies on unpublished siblings — the supported pattern is `path + version`, which means every "internal" microcrate becomes a real registry dependency the moment the parent publishes. There is no metadata trick to keep 30 crates "internal" while still publishing the umbrella. Either they're all real public products or they're not separate crates.

### 2.3 Strong architectural separation does not require crate boundaries

The architectural goal is **single-responsibility, low-coupling, layered modules**. Crate boundaries enforce this, but so do:
- folder-per-module structure
- `pub(crate)` visibility by default
- one-directional layered imports
- trait seams at layer boundaries
- per-folder `CLAUDE.md` for context locality

These give the same separation without the publish tax.

### 2.4 The dual implementation is real rot

Independent of the publish question, having `auth.rs` (1212 LOC) AND `auth_micro.rs` (333 LOC) AND `shipper-auth/src/lib.rs` (1762 LOC) — three implementations of token resolution, with the production path being the second + third — is a maintenance trap. Bug fixes in one path may not propagate; the in-tree version may quietly drift stale.

---

## 3. Target Public Crate Graph (13 crates)

```
                                    ┌─────────────────┐
                                    │  shipper-cli    │  binary, clap, output
                                    └────────┬────────┘
                                             ↓
                                    ┌─────────────────┐
                                    │     shipper     │  orchestration umbrella
                                    └────────┬────────┘
                                             ↓
        ┌──────────────────────────────────────────────────────────────┐
        ↓                ↓              ↓               ↓              ↓
┌──────────────┐ ┌──────────────┐ ┌──────────┐ ┌──────────────┐ ┌──────────────┐
│shipper-config│ │shipper-types │ │ leaves   │ │ utilities    │ │ integrations │
└──────┬───────┘ └──────┬───────┘ │ schema   │ │ duration     │ │ webhook      │
       │                 │         │ cargo-   │ │ retry        │ │ registry     │
       │                 │         │  failure │ │ encrypt      │ │ sparse-index │
       │                 │         │ output-  │ │              │ │              │
       │                 │         │  sanitiz.│ │              │ │              │
       └────→ shipper-types ←──────┴──────────┴──┴──────────────┴─┴──────────────┘
```

### 3.1 The 13 surviving crates — organized as TWO RINGS

The 13 published crates fall into two conceptually distinct rings. This separation matters for how we document, market, and support them: ring 1 is "the product story you tell users," ring 2 is "useful narrow seams that happen to be on crates.io."

#### Ring 1: real supported product surface (5 crates)

These are what users should think of as "Shipper." Documentation, examples, marketing, and the CLI's `--help` output should center on these.

| Crate | Class | Why it stays public |
|-------|-------|---------------------|
| `shipper-cli` | Product | Installed binary entry point |
| `shipper` | Product | Library API surface + orchestration umbrella |
| `shipper-config` | Contract | `.shipper.toml` schema + parsing/merging |
| `shipper-types` | Contract | Shared DTOs (ReleaseSpec, Receipt, etc.) embedders couple to |
| `shipper-schema` | Contract | State-file schema versioning *(audit in Phase 6 — may fold into `shipper-types::schema`, dropping ring 1 to 4 crates)* |

#### Ring 2: published support crates (8 crates)

These stay published because the boundary is narrow, useful, and cheap to keep stable — but they are NOT the product story. Users may pull them transitively or pick them up individually if they have a focused need. They get docs.rs pages and semver promises, but README narrative around them is minimal.

| Crate | Class | Why it stays public |
|-------|-------|---------------------|
| `shipper-duration` | Utility | Generic duration parsing — reusable |
| `shipper-retry` | Utility | Generic retry/backoff with jitter — reusable |
| `shipper-encrypt` | Utility | State file encryption — narrow, stable |
| `shipper-webhook` | Integration | Webhook delivery + HMAC signing — clean external seam |
| `shipper-registry` | Integration | Cargo registry API client — clean external seam |
| `shipper-sparse-index` | Integration | Sparse-index protocol — narrow, reusable |
| `shipper-cargo-failure` | Leaf | Cargo error classification — stable, reusable |
| `shipper-output-sanitizer` | Leaf | ANSI strip / output normalization — narrow leaf |

### 3.1.1 Why two rings, not one flat list

A boundary deserves to be a published crate when it is worth **a crates.io page, a semver promise, a permanently-owned name, and ongoing support**. Ring 1 crates clear that bar AND form the product narrative. Ring 2 crates clear the support bar but don't carry product narrative. Everything not in either ring becomes an SRP module under an owner crate — folder boundary instead of crate boundary, `pub(crate)` instead of `pub`, no semver tax.

The two-ring framing also satisfies the "slightly too microcrated" instinct in a healthy way: we keep 8 narrow leaves as published seams (more than a strict-minimum design would), but we stop pretending orchestration shards (`shipper-engine-parallel`, `shipper-plan`, `shipper-state`, `shipper-store`, etc.) are products.

### 3.2 The 17 absorbed crates

These become folders inside `shipper`, `shipper-config`, or `shipper-cli`:

**Into `shipper`:**
- `shipper-auth` → `shipper/src/ops/auth/`
- `shipper-cargo` → `shipper/src/ops/cargo/`
- `shipper-process` → `shipper/src/ops/process/`
- `shipper-git` → `shipper/src/ops/git/`
- `shipper-lock` → `shipper/src/ops/lock/`
- `shipper-environment` → `shipper/src/runtime/environment/`
- `shipper-policy` → `shipper/src/runtime/policy/`
- `shipper-execution-core` → `shipper/src/runtime/execution/`
- `shipper-state` → `shipper/src/state/execution_state/`
- `shipper-store` → `shipper/src/state/store/`
- `shipper-events` → `shipper/src/state/events/`
- `shipper-plan` → `shipper/src/plan/` (multiple submodules)
- `shipper-levels` → `shipper/src/plan/levels/`
- `shipper-chunking` → `shipper/src/plan/chunking/`
- `shipper-engine-parallel` → `shipper/src/engine/parallel/`

**Into `shipper-config`:**
- `shipper-config-runtime` → `shipper-config/src/runtime/`

**Into `shipper-cli`:**
- `shipper-progress` → `shipper-cli/src/output/progress/`

**SPLIT (NOT absorbed wholesale):**
- `shipper-storage` is **split by concern**, not collapsed into one folder:
  - **Storage config/data shapes** (e.g., `StorageConfig`, `BackendKind` enum, the public types embedders use to specify their storage backend) → move into `shipper-types::storage`
  - **Runtime storage/backend behavior** (the actual `StorageBackend` trait and the filesystem implementation) → move into `shipper/src/ops/storage/`
  - **Why split:** the crate's own `lib.rs` says only filesystem is fully implemented (S3/GCS/Azure bail out). It's not a finished standalone public product yet. Keeping config types in `shipper-types` means embedders can wire up their storage choice through the stable contract crate; keeping backend code in `shipper` means we don't make a public promise about a half-finished backend abstraction.

### 3.3 Open question to resolve before publish

**`shipper-schema` vs `shipper-types`.** If `shipper-schema` is purely versioning constants for the state-file format, it could be `shipper-types::schema` and we drop to **12 public crates**. Audit this before publishing.

---

## 4. Internal Module Architecture

### 4.1 The five-layer structure inside `shipper`

```
crates/shipper/src/
├── CLAUDE.md
├── lib.rs                              # facade: only re-exports + 5 mod decls
│
├── engine/                             # LAYER 5: orchestration (top)
│   ├── CLAUDE.md
│   ├── mod.rs                          # run_preflight, run_publish, run_resume
│   ├── preflight/
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── checks.rs
│   ├── publish/
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── retry_loop.rs
│   ├── parallel/                       ← shipper-engine-parallel (3237 LOC)
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   ├── scheduler.rs
│   │   ├── waves.rs
│   │   └── worker.rs
│   ├── resume/
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── reconcile.rs
│   └── readiness/
│       ├── CLAUDE.md
│       ├── mod.rs
│       ├── api.rs
│       └── sparse.rs
│
├── plan/                               # LAYER 4: planning algorithms
│   ├── CLAUDE.md
│   ├── mod.rs                          # build_plan, ReleasePlan
│   ├── filter.rs
│   ├── topo.rs                         # Kahn's algorithm (deterministic)
│   ├── levels/                         ← shipper-levels
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── grouping.rs
│   └── chunking/                       ← shipper-chunking
│       ├── CLAUDE.md
│       ├── mod.rs
│       └── splitter.rs
│
├── state/                              # LAYER 3: persistence
│   ├── CLAUDE.md
│   ├── mod.rs
│   ├── execution_state/                ← shipper-state
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── transitions.rs
│   ├── store/                          ← shipper-store
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   ├── trait_def.rs
│   │   └── fs.rs
│   ├── events/                         ← shipper-events
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── jsonl.rs
│   └── receipt/
│       ├── CLAUDE.md
│       ├── mod.rs
│       └── writer.rs
│
├── runtime/                            # LAYER 2: runtime context (pure data)
│   ├── CLAUDE.md
│   ├── mod.rs
│   ├── environment/                    ← shipper-environment
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── fingerprint.rs
│   ├── policy/                         ← shipper-policy
│   │   ├── CLAUDE.md
│   │   ├── mod.rs
│   │   └── presets.rs
│   └── execution/                      ← shipper-execution-core
│       ├── CLAUDE.md
│       ├── mod.rs
│       └── context.rs
│
└── ops/                                # LAYER 1: I/O primitives (bottom)
    ├── CLAUDE.md
    ├── mod.rs
    ├── auth/                           ← shipper-auth
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   ├── resolver.rs
    │   ├── credentials.rs
    │   └── oidc.rs
    ├── git/                            ← shipper-git
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   ├── cleanliness.rs
    │   └── context.rs
    ├── lock/                           ← shipper-lock
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   └── fs_lock.rs
    ├── process/                        ← shipper-process
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   └── spawn.rs
    ├── cargo/                          ← shipper-cargo
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   ├── metadata.rs
    │   └── publish.rs
    └── storage/                        ← shipper-storage
        ├── CLAUDE.md
        ├── mod.rs
        ├── trait_def.rs
        └── fs.rs
```

### 4.2 Architectural rules (binding)

**R1. Every absorbed microcrate becomes its own folder.**
The folder is the SRP boundary. Even single-implementation modules get a folder, because that's where the per-module `CLAUDE.md` lives.

**R2. `mod.rs` is the public-to-the-crate facade for its folder.**
Only items re-exported from `mod.rs` are visible outside the folder. Submodule files default to `pub(super)` or private. This is the analog of "no other crate touches your private items."

**R3. Layer dependencies are one-directional.**
- `engine` may import `plan`, `state`, `runtime`, `ops`
- `plan` may import `state`, `runtime`, `ops`
- `state` may import `runtime`, `ops`
- `runtime` may import `ops` (and pure data crates only)
- `ops` may import nothing from above

A grep-based CI check enforces this.

**R4. `pub(crate)` by default.**
Items at `lib.rs` are `pub`. Layer `mod.rs` files expose `pub(crate)` to their siblings. Nothing is `pub` unless it's deliberately part of the published surface.

**R5. Public types come from `shipper-types`.**
`shipper::types` is `pub use shipper_types::*;`. Embedders couple to `shipper-types`, never to `shipper`'s internal modules.

**R6. No `micro-*` features anywhere.**
Single canonical implementation per concept. No conditional module selection.

**R7. Folder depth cap: 3 inside any layer.**
`shipper/src/ops/auth/credentials.rs` — fine (depth 3).
`shipper/src/ops/auth/parser/toml/internal.rs` — banned (depth 5). At that point split into a sibling module.

**R8. Trait seams stay where they are.**
`StateStore`, `StorageBackend`, `Reporter`, `CommandRunner` — these traits exist because there are real swap points (mocks for testing, future cloud backends). They survive the absorption. Do **not** introduce *new* traits "to preserve the microcrate seam" if there's only one impl and one consumer.

### 4.3 Per-folder `CLAUDE.md` template

Each module folder gets a `CLAUDE.md` with:

1. **Single-responsibility statement** — one sentence
2. **Layer position** — what this module may import, what it must NOT import
3. **Public-to-crate surface** — names exposed via `mod.rs`
4. **Invariants & gotchas** — non-obvious constraints
5. **Cross-references** — upstream/downstream callers

Each absorbed microcrate's existing `README.md` (and any `CLAUDE.md`) seeds the new module's `CLAUDE.md`. Don't lose that documentation context.

### 4.4 `shipper-cli` and `shipper-config` internal trees

```
crates/shipper-cli/src/
├── CLAUDE.md
├── main.rs
├── cli/
│   ├── CLAUDE.md
│   ├── mod.rs
│   └── parser.rs
├── commands/
│   ├── CLAUDE.md
│   ├── mod.rs
│   ├── plan.rs, preflight.rs, publish.rs, resume.rs,
│   ├── status.rs, doctor.rs, inspect_events.rs,
│   ├── inspect_receipt.rs, clean.rs, config.rs
│   └── (folders only when a command grows past one file)
└── output/
    ├── CLAUDE.md
    ├── mod.rs
    ├── progress/                       ← shipper-progress
    │   ├── CLAUDE.md
    │   ├── mod.rs
    │   └── bar.rs
    ├── format/
    │   └── ...
    └── reporter/
        └── ...

crates/shipper-config/src/
├── CLAUDE.md
├── lib.rs
├── file/
│   ├── CLAUDE.md
│   ├── mod.rs
│   └── sections.rs
├── merge/
│   ├── CLAUDE.md
│   ├── mod.rs
│   └── overrides.rs
├── validate/
│   ├── CLAUDE.md
│   ├── mod.rs
│   └── invariants.rs
└── runtime/                            ← shipper-config-runtime
    ├── CLAUDE.md
    ├── mod.rs
    └── conversion.rs
```

---

## 5. Per-Subsystem Audit Findings

The audit revealed which implementation is canonical for each subsystem (the one that production currently runs via `micro-all`):

| Subsystem | In-tree LOC | Shim LOC | Crate LOC | Canonical | Absorption complexity |
|-----------|-------------|----------|-----------|-----------|----------------------|
| `auth` | 1212 | 333 | 1762 | shim+crate (merge) | **Hard** — shim has fallback credential parsing |
| `cargo` | 1175 | 4 | 1450 | crate | Easy — pure re-export shim |
| `process` | 105 | 32 | 1948 | crate | Easy |
| `engine_parallel` | 3237 | 41 | N/A | **in-tree only** | Easy — just delete shim referencing nothing |
| `environment` | 190 | 79 | 2202 | crate (with shim adjustments) | Medium |
| `events` | 354 | 1 | 2821 | crate | Easy — pure re-export |
| `git` | 1115 | 158 | 2095 | crate (with `SHIPPER_GIT_BIN` override from shim) | Medium |
| `lock` | 337 | 1 | 2059 | crate | Easy — pure re-export |
| `plan` | 1584 | 1 | 3492 | crate | Easy |
| `policy` | 168 | 7 | 1040 | crate (with thin shim) | Easy |
| `registry` | 4791 | 239 | 1293 | **in-tree** (4x larger than crate) | **Special** — see §5.1 |
| `state` | 1566 | 1 | 2689 | crate | Easy |
| `store` | 386 | 1 | 2816 | crate | Easy |
| `storage` | 372 | 153 | 1664 | crate (with `base_path` shim wrapper) | Medium |

### 5.1 Special case: `shipper-registry`

The in-tree `crates/shipper/src/registry.rs` (4791 LOC) is **4x larger** than the public `shipper-registry` crate (1293 LOC). The in-tree version contains logic the public crate does not: ownership queries, manifest caching, credential interop.

**Resolution:** Since `shipper-registry` stays public, the in-tree logic must move INTO `crates/shipper-registry/` so the public crate is functionally complete. After this move, `shipper` depends on `shipper-registry` and re-exports what it needs. There is no separate `shipper/src/ops/registry/` folder.

### 5.2 Total LOC at risk of divergence

~1,400 LOC across `auth`, `git`, `registry`, `storage`, and `engine_parallel`. These are the merges where the shim has non-trivial logic on top of the microcrate; the merge must preserve both code paths (or consciously drop the shim's fallback if it's obsolete).

---

## 6. Migration Phases

Each phase is committed as one or more atomic commits. Each phase has a hard validation gate before moving to the next.

### Phase 0: Setup (one PR)

- Create `feature/decrating` branch from current main
- Add this planning doc (this file) to `docs/`
- Add CI check for one-direction layer imports (will be a no-op until layers exist)
- **Validation gate:** `cargo test --workspace` passes

### Phase 1: Scaffold the five-layer structure (one PR)

Create the layer dirs and `mod.rs` + `CLAUDE.md` files inside `shipper/src/`:

```
crates/shipper/src/
├── engine/        (mod.rs + CLAUDE.md, no submodules yet)
├── plan/
├── state/
├── runtime/
└── ops/
```

Each new folder gets a placeholder `CLAUDE.md` with its layer description and import rules. The `mod.rs` files are empty stubs (no `pub mod` declarations yet) — they exist so the architecture-guard CI workflow becomes active.

**No code is moved yet.** This is purely structural scaffolding.

**Validation gate:** Workspace still compiles; CI grep-check for upward imports is now active.

### Phase 2: Per-subsystem absorption (one PR per microcrate, partially parallelizable)

> **EXECUTION REVISION (2026-04-15):** The original plan had three separate phases (Phase 1 dedup, Phase 4 move into folder, Phase 5 absorb microcrate). They are now **merged into a single per-subsystem operation** because (a) the in-tree `<name>.rs` is virtually always a stale duplicate of the canonical microcrate version — confirmed by the audit and by the user, and (b) splitting into three PRs per subsystem creates 50+ PRs with mid-state stages that compile but expose meaningless intermediate APIs. One PR per subsystem keeps `git bisect` useful and PR review tractable.

For each absorbed microcrate, **one atomic PR** does all of the following:

1. **Determine canonical version.** Per the audit (§5), the standalone microcrate is canonical for nearly all subsystems. The in-tree `<name>.rs` is a stale duplicate. Exceptions:
   - Some shims (`auth_micro.rs`, `git_micro.rs`, `storage_micro.rs`, `environment_micro.rs`) wrap the microcrate with additional logic (credential fallback, env overrides, compatibility wrappers) that must be preserved into the absorbed module.
   - `engine_parallel` has both a 3237-LOC in-tree and a 4826-LOC standalone — the standalone is canonical (larger, has `webhook.rs` submodule, snapshot tests, BDD tests). See §5.1 for special handling.

2. **Create the target folder** under the appropriate layer (per §3.2 ownership map):
   ```
   crates/shipper/src/<layer>/<name>/
   ├── CLAUDE.md       (seeded from the microcrate's README.md + shim's notes)
   ├── mod.rs          (the public-to-crate facade — pub(crate) by default)
   ├── <split files>   (large modules split per Rule R7: depth cap 3)
   └── tests.rs        (or inline #[cfg(test)] mod tests)
   ```

3. **Move the canonical source into the folder:**
   - For pure re-export shims: copy `crates/shipper-<name>/src/lib.rs` content into `crates/shipper/src/<layer>/<name>/mod.rs`, splitting into sub-files if >500 LOC.
   - For shims with added logic: merge the microcrate's source with the shim's added functions into the new `mod.rs` (or split sub-files).

4. **Move tests** from `crates/shipper-<name>/src/lib.rs` (inline `#[cfg(test)] mod tests`) and `crates/shipper-<name>/tests/` (integration tests) into the new folder. Inline unit tests stay inline; integration tests move to `tests.rs` co-located with the module.

5. **Move snapshots.** If the microcrate has `crates/shipper-<name>/src/snapshots/`, relocate them to the new folder. Insta paths are sensitive — run `cargo insta accept` after the move and verify diffs are only path-related.

6. **Update imports across the workspace.** All `use shipper_<name>::X` becomes `use crate::<layer>::<name>::X` inside `shipper`, or `use shipper::<layer>::<name>::X` if exported (which it shouldn't be — most absorbed modules are `pub(crate)`).

7. **Delete the standalone microcrate:**
   ```bash
   git rm -r crates/shipper-<name>/
   ```

8. **Update workspace state:**
   - Remove `"crates/shipper-<name>"` from root `Cargo.toml` workspace members
   - Remove `shipper-<name> = { ... }` from `crates/shipper/Cargo.toml` dependencies
   - Delete the `micro-<name>` feature from `crates/shipper/Cargo.toml` features section
   - Delete the `micro-<name>` feature from `crates/shipper-cli/Cargo.toml` and remove from the `micro-all` list

9. **Delete the in-tree stale duplicate** (if it still exists):
   ```bash
   git rm crates/shipper/src/<name>.rs
   git rm crates/shipper/src/<name>_micro.rs
   ```

10. **Update `crates/shipper/src/lib.rs`** — remove the cfg-gated module decl for `<name>` (the module now lives inside its layer's `mod.rs`).

11. **Update the layer's `mod.rs`** — add `pub(crate) mod <name>;` (or `pub mod <name>;` if it's part of `shipper`'s public surface).

12. **Validation per PR:**
    ```bash
    cargo check --workspace
    cargo test -p shipper <name>
    cargo test -p shipper
    cargo test -p shipper-cli
    cargo build -p shipper-cli
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    ```

13. **One commit per microcrate.** No squashing across subsystems.

**Order (parallelizable across non-overlapping layers):**

| Wave | Subsystems | Layer | Notes |
|------|------------|-------|-------|
| 1 | `lock`, `process` | `ops/` | Easy — pure re-exports, no shim logic to merge |
| 1 | `events` | `state/` | Easy |
| 1 | `levels`, `chunking` | `plan/` | Easy — algorithm crates, no in-tree counterparts |
| 2 | `cargo`, `storage`, `git` | `ops/` | Medium — shim has logic to preserve |
| 2 | `state`, `store` | `state/` | Easy but large; needs sub-file split |
| 2 | `policy`, `environment` | `runtime/` | Mixed |
| 3 | `auth` | `ops/` | Hard — credential fallback |
| 3 | `plan` | `plan/` | Large, sub-file split |
| 3 | `execution-core` | `runtime/` | No in-tree counterpart |
| 4 | `engine_parallel` | `engine/` | Hardest — see §5.1 + handle `fuzz` consumer |

Within a wave, agents can work in parallel on disjoint subsystems. Between waves, validate.

### Phase 3: Drop `micro-all` default + delete all `micro-*` features (one PR)

After Phase 2 fully clears the absorbed microcrates, the `micro-*` feature flags are no-ops or refer to deleted deps. Now:

1. In `shipper-cli/Cargo.toml`, remove `default = ["micro-all"]` and all `micro-*` feature passthrough entries
2. In `shipper/Cargo.toml`, delete every remaining `micro-*` feature definition
3. Grep the entire repo for `micro-` references and clean up CI workflows, README examples, `.shipper.toml` files

**Validation gate:** `cargo test --workspace` passes; `cargo build -p shipper-cli` produces a binary that runs end-to-end against a test workspace.

### Phase 4: Special case — fold in-tree `registry` logic into `shipper-registry` (one PR)

1. Move logic from `crates/shipper/src/registry.rs` (which is now in some layer, possibly `ops/registry/` if Phase 4 placed it there) INTO `crates/shipper-registry/src/`, splitting into `api.rs`, `ownership.rs`, `manifest_cache.rs`, `credentials.rs`
2. Delete the in-tree `registry/` folder (or `registry.rs`) from `shipper`
3. `shipper` now depends on `shipper-registry` only — no internal wrapper
4. Update import sites

**Validation gate:** `cargo test -p shipper`, `cargo test -p shipper-registry`, `cargo build -p shipper-cli`.

### Phase 5: Absorb adapters into config and CLI (one PR per absorption)

- `shipper-config-runtime` → `shipper-config/src/runtime/`
- `shipper-progress` → `shipper-cli/src/output/progress/`

**Validation gate:** workspace tests pass.

### Phase 6: Resolve `shipper-schema` vs `shipper-types`

Audit overlap. If schema is purely versioning constants, fold into `shipper-types::schema` and drop `shipper-schema`. Otherwise keep both. Document the decision.

**Validation gate:** workspace tests pass; `cargo public-api --diff` shows no unintended public-API expansion.

### Phase 7: Convert surviving deps to `path + version` and add `default-members`

Root `Cargo.toml`:
```toml
[workspace]
members = [
  "crates/shipper",
  "crates/shipper-cli",
  "crates/shipper-types",
  "crates/shipper-config",
  "crates/shipper-schema",        # if it survives Phase 8
  "crates/shipper-duration",
  "crates/shipper-retry",
  "crates/shipper-encrypt",
  "crates/shipper-webhook",
  "crates/shipper-registry",
  "crates/shipper-sparse-index",
  "crates/shipper-cargo-failure",
  "crates/shipper-output-sanitizer",
]
default-members = ["crates/shipper-cli", "crates/shipper"]

[workspace.dependencies]
shipper-types = { path = "crates/shipper-types", version = "0.3.0-rc.1" }
shipper-config = { path = "crates/shipper-config", version = "0.3.0-rc.1" }
# ... etc for all 13 ...
```

Each member's `Cargo.toml` uses `dep.workspace = true`.

**Validation gate:** `cargo package --list -p <crate>` for each public crate; inspect tarball contents; `cargo publish --dry-run -p <crate>` in topo order.

### Phase 8: Publish dry-run and release (one PR)

- Run `cargo publish --dry-run` for all 13 crates in topo order
- Update `RELEASE_CHECKLIST_v0.3.0.md` with the new publish sequence
- Cargo 1.90 multi-package publish is available (`cargo publish --workspace`) but is **non-atomic** — partial publish failures must be recoverable. Document the recovery procedure in the release checklist.

**Topological publish order:**
```
1.  shipper-duration
2.  shipper-retry
3.  shipper-encrypt
4.  shipper-output-sanitizer
5.  shipper-cargo-failure
6.  shipper-sparse-index
7.  shipper-webhook
8.  shipper-types
9.  shipper-schema           (if it survives Phase 8)
10. shipper-registry
11. shipper-config
12. shipper
13. shipper-cli
```

---

## 7. Hazards & Mitigations (Learnings)

### 7.1 Tests inside microcrates need a destination
Each absorbed microcrate has unit tests. Plan per crate: unit tests inline as `#[cfg(test)] mod tests`, integration tests fold into `crates/shipper/tests/`. **Don't lose them** — some are the only coverage for edge cases (e.g., `[registries.crates.io]` nested-table TOML parsing in `auth_micro.rs`).

### 7.2 Snapshots travel with their tests
`crates/shipper-lock/src/snapshots/` and similar exist. Insta snapshot files are path-sensitive. Run `cargo insta accept` after each absorption batch to refresh paths.

### 7.3 Doc tests in absorbed crates' `lib.rs` will silently break
Doc examples like `use shipper_auth::resolve_token` need rewriting to `use shipper::ops::auth::resolve_token`. Easy to miss until `cargo test --doc` runs. Always run `--doc` in validation gates.

### 7.4 Feature flag deletion is not free
Removing `micro-*` features means any external consumer (CI scripts, README examples, `.shipper.toml`, GitHub Actions workflows) that references those features breaks. Grep `micro-` across the **entire repo**, not just `Cargo.toml` files.

### 7.5 `cargo_metadata`-driven self-tests change behavior
`shipper`'s plan-builder reads the workspace via `cargo_metadata`. After collapse, the workspace has 13 members instead of 30. Any test fixture asserting "the workspace contains N publishable crates" needs updating. Self-referential tests where `shipper` plans publishing of `shipper`'s own workspace will change.

### 7.6 Boundary enforcement that makes the architecture stick
- **CI check for upward imports** — grep-based: fail if `crates/shipper/src/ops/**/*.rs` contains `use crate::engine::` etc.
- **`#![deny(unused_crate_dependencies)]`** per published crate — catches stale deps after collapse
- **`cargo public-api --diff`** against a pre-collapse baseline — catches accidental public-API expansion

### 7.7 Cargo / publishing logistics
- `cargo package --list` shows what would be packaged but is not byte-identical to the final tarball; inspect with `tar -tzf target/package/<crate>.crate`
- Publish order is topological **and** gated by registry visibility — wait for sparse-index propagation between layers
- Cargo 1.90 multi-package publish is non-atomic; document recovery
- Crates.io is immutable: anything published stays published. Yank, don't try to "remove"

### 7.8 docs.rs metadata
`[package.metadata.docs.rs]` in absorbed microcrates may have feature flags or build args that need merging into `shipper/Cargo.toml`'s docs.rs config. Audit each absorbed crate's `Cargo.toml`.

### 7.9 `.shipper.toml` config-schema compatibility
If absorbed microcrates contributed config sections (e.g., `[storage]`, `[lock]`), the config-loading code path moves. Existing `.shipper.toml` files in user repos must still parse. Add a config compatibility test that loads a frozen pre-collapse example and verifies it works.

### 7.10 Process discipline
- **One commit per absorption.** 17+ commits, not one giant Phase 3 commit. `git bisect` must work.
- **Each absorbed microcrate's docs migrate in the same commit as the code.** Don't defer doc migration to "after."
- **Run the CLI binary end-to-end after each absorption batch.** Type checks verify code; only running the actual binary verifies integration.

### 7.11 What NOT to do
- **Do not introduce trait-based abstractions "just to preserve the microcrate seam."** If there's only one impl and one consumer, the trait is dead weight. Folder + `pub(crate)` is enough separation. Add traits only where multiple impls actually exist.
- **Do not collapse responsibilities.** SRP-by-microcrate → SRP-by-module is one-for-one. Don't merge `auth` and `credentials` "since they're related."
- **Do not skip `--no-default-features` validation.** It's the path that tested the in-tree implementation pre-collapse; keep it green even though the feature distinction goes away, to catch any cfg-gated code that escaped the cleanup.

---

## 8. Validation Gates Summary

Each phase exit requires:

| Gate | Command | When |
|------|---------|------|
| Workspace builds | `cargo check --workspace` | Every commit |
| Workspace tests pass | `cargo test --workspace` | Every phase |
| Doc tests pass | `cargo test --workspace --doc` | Every absorption |
| Clippy clean | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | End of phase |
| CLI binary runs | `cargo run -p shipper-cli -- plan --dry-run` against fixture | End of phase |
| Layer imports clean | `! grep -r "use crate::\(engine\|plan\|state\|runtime\)::" crates/shipper/src/ops/` | After Phase 3 |
| Public API stable | `cargo public-api --diff` against baseline | End of Phase 8 |
| Package contents correct | `cargo package -p <crate>` + tarball inspection | Phase 9 |
| Publish dry-runs pass | `cargo publish --dry-run -p <crate>` topo order | Phase 10 |

---

## 9. Open Questions

1. **`shipper-schema` vs `shipper-types`:** Resolve in Phase 8.
2. **`shipper-engine-parallel` published status:** Audit found "no microcrate referenced" but the workspace lists `crates/shipper-engine-parallel`. Confirm whether this crate has ever been published or is purely a workspace member with no production consumer.
3. **MSRV impact:** Workspace MSRV is 1.92. Any absorbed microcrate using newer features will fail. Verify in Phase 1.
4. **`unsafe_code = "forbid"`:** Workspace-wide. Verify no absorbed microcrate uses unsafe.
5. **Yank policy for already-published microcrates:** If any absorbed microcrate was published to crates.io, decide whether to publish a final yanked version with a "moved into shipper" notice in its README.
6. **Branch strategy:** One huge `feature/decrating` branch, or merge each phase to main as it lands? Recommend: branch per phase, merge as each validation gate passes — keeps `main` shippable throughout.

---

## 10. Estimated Effort

- **Phase 0 (setup):** 1 hour
- **Phase 1 (eliminate dual implementations, 14 subsystems):** 8-12 hours, parallelizable
- **Phase 2 (drop `micro-all`):** 1 hour
- **Phase 3 (scaffold layers):** 1 hour
- **Phase 4 (move flat files into layers):** 4-6 hours
- **Phase 5 (absorb 17 microcrates):** 12-16 hours, parallelizable per crate
- **Phase 6 (registry special case):** 3-4 hours
- **Phase 7 (config-runtime, progress):** 2 hours
- **Phase 8 (schema audit):** 1-2 hours
- **Phase 9 (deps, default-members, dry-runs):** 2-3 hours
- **Phase 10 (publish):** 1-2 hours

**Total: ~35-50 hours of focused work.** Heavy agent use brings this down significantly. Realistic calendar time: 1-2 weeks if done in dedicated sessions; longer if interleaved with other work.

---

## 11. Done Criteria

- [ ] Workspace has exactly 12 or 13 published crates (depending on Phase 8 outcome)
- [ ] Zero `*_micro.rs` files
- [ ] Zero `micro-*` feature flags
- [ ] `shipper/src/` has the five-layer structure with one folder per absorbed microcrate
- [ ] Every module folder has a `CLAUDE.md`
- [ ] Layer-import CI check is green and active
- [ ] All public crates pass `cargo publish --dry-run` in topo order
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] CLI binary runs end-to-end against a fixture workspace
- [ ] Public API surface has not unintentionally expanded (`cargo public-api --diff` clean)
- [ ] Release checklist updated with new publish sequence
- [ ] `RELEASE_NOTES_v0.3.0.md` documents the consolidation as a breaking-change-for-internal-users note
