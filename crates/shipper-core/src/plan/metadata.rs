use std::path::Path;

use anyhow::Result;
use cargo_metadata::Metadata;

pub(super) fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    crate::ops::cargo::load_metadata(manifest_path)
}
