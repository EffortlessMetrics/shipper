use std::path::Path;

use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::types::{ExecutionState, PackageReceipt, RuntimeOptions};
use shipper_engine_parallel as micro;
use shipper_registry;

/// Reporter implementation is intentionally delegated from the host crate so `shipper` can keep a
/// single reporting interface while reusing the dedicated parallel execution engine.
struct ReporterAdapter<'a> {
    inner: &'a mut dyn crate::engine::Reporter,
}

impl<'a> micro::Reporter for ReporterAdapter<'a> {
    fn info(&mut self, msg: &str) {
        self.inner.info(msg);
    }
    fn warn(&mut self, msg: &str) {
        self.inner.warn(msg);
    }
    fn error(&mut self, msg: &str) {
        self.inner.error(msg);
    }
}

pub use micro::chunk_by_max_concurrent;

/// Run publish in parallel mode, processing dependency levels sequentially and packages
/// within each level concurrently.
pub fn run_publish_parallel(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    reporter: &mut dyn crate::engine::Reporter,
) -> anyhow::Result<Vec<PackageReceipt>> {
    let reg = shipper_registry::RegistryClient::new(&reg.registry().api_base);
    let mut adapter = ReporterAdapter { inner: reporter };
    micro::run_publish_parallel(ws, opts, st, state_dir, &reg, &mut adapter)
}
