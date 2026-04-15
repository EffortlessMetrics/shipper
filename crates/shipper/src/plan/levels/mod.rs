//! Wave grouping for parallel publish.
//!
//! Re-exports the generic level-grouping algorithm from `shipper_types::levels`.
//! The algorithm was previously its own standalone crate (`shipper-levels`);
//! it was absorbed into `shipper-types` in the same PR that created this
//! module, because `shipper_types::ReleasePlan::group_by_levels` is its
//! primary consumer and `shipper-types` can't depend on `shipper`.
//!
//! This thin re-export keeps the `shipper::plan::levels` path stable and
//! matches the decrating layer-dir convention.

pub use shipper_types::levels::{PublishLevel, group_packages_by_levels};
