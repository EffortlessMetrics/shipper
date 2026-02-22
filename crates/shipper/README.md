# shipper (library)

`shipper` is the core library for reliable, resumable Rust workspace publishing.
It powers `shipper-cli` and is useful when you want to embed publish orchestration
into custom release tooling.

## What this crate does

- Builds deterministic publish plans from workspace metadata.
- Runs preflight checks (git state, publishability, registry visibility, ownership checks).
- Executes publish flows with retry and backoff.
- Verifies registry visibility and readiness between dependency levels.
- Persists state, receipts, and event logs for resumable execution.
- Supports sequential and parallel publishing engines.

## Public API map

- `plan::build_plan` - build the dependency-first publish plan.
- `engine::run_preflight` - run checks without publishing.
- `engine::run_publish` - execute publish with state persistence.
- `engine::run_resume` - continue interrupted runs.
- `engine_parallel::run_publish_parallel` - publish dependency levels concurrently.
- `config` - load and merge `.shipper.toml` settings.
- `types` - domain types for plans, options, state, events, and receipts.

## Minimal integration example

```rust,no_run
use anyhow::Result;
use shipper::config::{CliOverrides, ShipperConfig};
use shipper::engine::{self, Reporter};
use shipper::plan;
use shipper::types::{Registry, ReleaseSpec};

struct StdReporter;

impl Reporter for StdReporter {
    fn info(&mut self, msg: &str) {
        eprintln!("[info] {msg}");
    }

    fn warn(&mut self, msg: &str) {
        eprintln!("[warn] {msg}");
    }

    fn error(&mut self, msg: &str) {
        eprintln!("[error] {msg}");
    }
}

fn main() -> Result<()> {
    let spec = ReleaseSpec {
        manifest_path: "Cargo.toml".into(),
        registry: Registry::crates_io(),
        selected_packages: None,
    };

    let planned = plan::build_plan(&spec)?;
    let opts = ShipperConfig::default().build_runtime_options(CliOverrides::default());
    let mut reporter = StdReporter;

    let report = engine::run_preflight(&planned, &opts, &mut reporter)?;
    println!("Finishability: {:?}", report.finishability);
    Ok(())
}
```

## Not in scope

`shipper` does not decide version numbers, generate changelogs, tag releases,
or create GitHub releases. Pair it with your preferred versioning/release
workflow and use this crate to make publishing reliable.

## More documentation

- Project overview: <https://github.com/EffortlessMetrics/shipper#readme>
- Configuration reference: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/configuration.md>
- Failure modes: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/failure-modes.md>
