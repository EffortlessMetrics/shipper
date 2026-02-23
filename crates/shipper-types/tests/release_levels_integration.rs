use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;
use shipper_types::{PlannedPackage, Registry, ReleasePlan};

fn pkg(name: &str) -> PlannedPackage {
    PlannedPackage {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        manifest_path: PathBuf::from(format!("crates/{name}/Cargo.toml")),
    }
}

#[test]
fn release_plan_groups_parallel_levels_from_shared_microcrate_logic() {
    let plan = ReleasePlan {
        plan_version: "shipper.plan.v1".to_string(),
        plan_id: "test-plan".to_string(),
        created_at: Utc::now(),
        registry: Registry::crates_io(),
        packages: vec![pkg("core"), pkg("api"), pkg("cli"), pkg("app")],
        dependencies: BTreeMap::from([
            ("core".to_string(), Vec::new()),
            ("api".to_string(), vec!["core".to_string()]),
            ("cli".to_string(), vec!["core".to_string()]),
            (
                "app".to_string(),
                vec!["api".to_string(), "cli".to_string()],
            ),
        ]),
    };

    let levels = plan.group_by_levels();
    assert_eq!(levels.len(), 3);
    assert_eq!(
        levels[0]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["core"]
    );
    assert_eq!(
        levels[1]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["api", "cli"]
    );
    assert_eq!(
        levels[2]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["app"]
    );
}
