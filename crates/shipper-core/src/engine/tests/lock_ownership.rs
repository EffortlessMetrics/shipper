use super::*;

#[test]
#[serial]
fn held_ownership_refuses_publish_and_resume_without_mutating_artifacts_or_starting_cargo() {
    for parallel in [false, true] {
        for force in [false, true] {
            let directory = tempdir().unwrap();
            let bin = directory.path().join("bin");
            write_fake_tools(&bin);
            let cargo_log = directory.path().join("cargo-calls.log");
            let mut variables = fake_program_env_vars(&bin);
            variables.push((
                "SHIPPER_CARGO_ARGS_LOG",
                Some(cargo_log.to_string_lossy().into_owned()),
            ));
            temp_env::with_vars(variables, || {
                let ws = planned_workspace(directory.path(), "http://127.0.0.1:9".into());
                let state_dir = directory.path().join(".shipper");
                init_state(&ws, &state_dir).unwrap();
                let reconciliation = crate::types::ReconciliationReport {
                    schema_version: crate::state::reconciliation::RECONCILIATION_SCHEMA_VERSION
                        .into(),
                    plan_id: ws.plan.plan_id.clone(),
                    registry: ws.plan.registry.clone(),
                    generated_at: Utc::now(),
                    evidence_sources: Vec::new(),
                    records: Vec::new(),
                };
                fs::write(
                    state::reconciliation_path(&state_dir),
                    serde_json::to_vec_pretty(&reconciliation).unwrap(),
                )
                .unwrap();
                let event = crate::types::PublishEvent {
                    timestamp: Utc::now(),
                    event_type: crate::types::EventType::PlanCreated {
                        plan_id: ws.plan.plan_id.clone(),
                        package_count: ws.plan.packages.len(),
                    },
                    package: "all".into(),
                };
                fs::write(
                    events::events_path(&state_dir),
                    format!("{}\n", serde_json::to_string(&event).unwrap()),
                )
                .unwrap();
                fs::write(state::receipt_path(&state_dir), b"retained receipt bytes\n").unwrap();
                let owner =
                    crate::lock::LockFile::acquire(&state_dir, Some(&ws.workspace_root)).unwrap();
                owner.set_plan_id(&ws.plan.plan_id).unwrap();
                let lock_path = crate::lock::lock_path(&state_dir, Some(&ws.workspace_root));
                // Age cannot authorize --force to steal this actual handle.
                let mut metadata: serde_json::Value =
                    serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
                metadata["acquired_at"] =
                    serde_json::to_value(Utc::now() - chrono::Duration::days(30)).unwrap();
                fs::write(&lock_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
                let retained: Vec<_> = [
                    state::state_path(&state_dir),
                    events::events_path(&state_dir),
                    state::receipt_path(&state_dir),
                    state::reconciliation_path(&state_dir),
                    lock_path,
                ]
                .into_iter()
                .map(|path| {
                    let bytes = fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect();
                let mut opts = default_opts(state_dir);
                opts.parallel.enabled = parallel;
                opts.force = force;
                opts.lock_timeout = Duration::ZERO;
                for resume in [false, true] {
                    let mut reporter = CollectingReporter::default();
                    let result = if resume {
                        run_resume(&ws, &opts, &mut reporter)
                    } else {
                        run_publish(&ws, &opts, &mut reporter)
                    };
                    assert!(
                        result.is_err(),
                        "a held OS guard must refuse every entry path"
                    );
                    for (path, bytes) in &retained {
                        assert_eq!(
                            &fs::read(path).unwrap(),
                            bytes,
                            "refusal mutated {}",
                            path.display()
                        );
                    }
                    assert!(
                        !cargo_log.exists(),
                        "no Cargo process may start before ownership"
                    );
                }
                owner.release().unwrap();
            });
        }
    }
}
