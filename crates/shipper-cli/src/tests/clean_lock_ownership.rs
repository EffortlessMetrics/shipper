use super::*;

fn retained_artifacts(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fs::create_dir_all(directory.join("cache")).unwrap();
    [
        "state.json",
        "events.jsonl",
        "receipt.json",
        "reconciliation.json",
        "preflight-only-proof.events.jsonl",
        "cache/retained-index",
    ]
    .into_iter()
    .map(|name| {
        let path = directory.join(name);
        let bytes = format!("retained bytes for {name}\n").into_bytes();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    })
    .collect()
}

fn assert_preserved(retained: &[(PathBuf, Vec<u8>)]) {
    for (path, expected) in retained {
        assert_eq!(
            &fs::read(path).unwrap(),
            expected,
            "refused clean mutated {}",
            path.display()
        );
    }
}

fn multi_directory_artifacts(state_dir: &Path) -> (Vec<PathBuf>, Vec<(PathBuf, Vec<u8>)>) {
    let directories = vec![
        state_dir.to_path_buf(),
        state_dir.join("alpha"),
        state_dir.join("omega"),
    ];
    let retained = directories
        .iter()
        .flat_map(|directory| retained_artifacts(directory))
        .collect();
    (directories, retained)
}

fn assert_run_clean_preserved(retained: &[(PathBuf, Vec<u8>)]) {
    for (path, expected) in retained {
        assert_eq!(
            fs::read(path).ok().as_ref(),
            Some(expected),
            "run_clean must preserve every selected directory on refusal: {}",
            path.display()
        );
    }
}

#[test]
#[serial_test::serial]
fn run_clean_later_registry_hold_preserves_all_selected_evidence() {
    for event_only in [false, true] {
        for force in [false, true] {
            for keep_receipt in [false, true] {
                let directory = tempdir().unwrap();
                let state_dir = directory.path().join(".shipper");
                let (directories, paths) = multi_directory_artifacts(&state_dir);
                let held_dir = &directories[2];
                let hold = "cleanup could not be confirmed";
                if event_only {
                    let event = PublishEvent {
                        timestamp: Utc::now(),
                        package: "held@0.1.0".into(),
                        event_type: EventType::PackageCleanupIncomplete {
                            message: hold.into(),
                        },
                    };
                    fs::write(
                        held_dir.join("events.jsonl"),
                        format!("{}\n", serde_json::to_string(&event).unwrap()),
                    )
                    .unwrap();
                } else {
                    let now = Utc::now();
                    let state = ExecutionState {
                        state_version: "shipper.state.v1".into(),
                        plan_id: "retained-cleanup-plan".into(),
                        registry: Registry::crates_io(),
                        created_at: now,
                        updated_at: now,
                        attempt_history: Vec::new(),
                        packages: BTreeMap::from([(
                            "held@0.1.0".into(),
                            PackageProgress {
                                name: "held".into(),
                                version: "0.1.0".into(),
                                attempts: 1,
                                state: PackageState::CleanupIncomplete {
                                    message: hold.into(),
                                },
                                last_updated_at: now,
                            },
                        )]),
                    };
                    fs::write(
                        held_dir.join("state.json"),
                        serde_json::to_vec(&state).unwrap(),
                    )
                    .unwrap();
                }
                let retained: Vec<_> = paths
                    .into_iter()
                    .map(|(path, _)| {
                        let bytes = fs::read(&path).unwrap();
                        (path, bytes)
                    })
                    .collect();

                let error = run_clean(&state_dir, directory.path(), keep_receipt, force, "text")
                    .unwrap_err();
                assert_eq!(
                    engine::classify_publish_stop(&error),
                    Some(engine::PublishStopClassification::CleanupIncomplete)
                );
                assert_run_clean_preserved(&retained);
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn run_clean_later_registry_owner_preserves_all_selected_evidence() {
    for missing_metadata in [false, true] {
        for force in [false, true] {
            for keep_receipt in [false, true] {
                let directory = tempdir().unwrap();
                let state_dir = directory.path().join(".shipper");
                let (directories, mut retained) = multi_directory_artifacts(&state_dir);
                let held_dir = &directories[2];
                let owner = shipper_core::lock::LockFile::acquire(held_dir, Some(directory.path()))
                    .unwrap();
                let lock_path = shipper_core::lock::lock_path(held_dir, Some(directory.path()));
                if missing_metadata {
                    fs::remove_file(&lock_path).unwrap();
                } else {
                    retained.push((lock_path.clone(), fs::read(&lock_path).unwrap()));
                }

                let error = run_clean(&state_dir, directory.path(), keep_receipt, force, "json")
                    .unwrap_err();
                assert!(format!("{error:#}").contains("OS ownership is active"));
                assert_run_clean_preserved(&retained);
                assert_eq!(lock_path.exists(), !missing_metadata);
                owner.release().unwrap();
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn run_clean_multiple_directories_preserves_only_requested_receipts() {
    for force in [false, true] {
        for keep_receipt in [false, true] {
            let directory = tempdir().unwrap();
            let state_dir = directory.path().join(".shipper");
            let (directories, retained) = multi_directory_artifacts(&state_dir);

            run_clean(&state_dir, directory.path(), keep_receipt, force, "json").unwrap();

            for (path, expected) in retained {
                let kept = keep_receipt
                    && matches!(
                        path.file_name().and_then(|name| name.to_str()),
                        Some("receipt.json" | "reconciliation.json")
                    );
                if kept {
                    assert_eq!(fs::read(&path).unwrap(), expected);
                } else {
                    assert!(!path.exists(), "clean left {}", path.display());
                }
            }
            for cleaned_dir in directories {
                let lock_path = shipper_core::lock::lock_path(&cleaned_dir, Some(directory.path()));
                assert!(!lock_path.exists());
                let mut guard_path = lock_path.into_os_string();
                guard_path.push(".guard");
                assert!(Path::new(&guard_path).exists());
                shipper_core::lock::LockFile::acquire(&cleaned_dir, Some(directory.path()))
                    .unwrap()
                    .release()
                    .unwrap();
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn run_clean_holds_every_selected_owner_through_the_last_deletion() {
    struct ObserverGuard;
    impl Drop for ObserverGuard {
        fn drop(&mut self) {
            CLEAN_DELETE_OBSERVER.with(|observer| observer.replace(None));
        }
    }

    let directory = tempdir().unwrap();
    let state_dir = directory.path().join(".shipper");
    let (directories, retained) = multi_directory_artifacts(&state_dir);
    let workspace_root = directory.path().to_path_buf();
    let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observations = std::rc::Rc::clone(&observed);
    let selected = directories.clone();
    CLEAN_DELETE_OBSERVER.with(|observer| {
        observer.replace(Some(Box::new(move |current| {
            for selected_dir in &selected {
                let error =
                    shipper_core::lock::LockFile::acquire(selected_dir, Some(&workspace_root))
                        .expect_err("every selected directory must remain exclusively owned");
                assert!(format!("{error:#}").contains("OS ownership is active"));
            }
            if observations.borrow().is_empty() {
                assert_run_clean_preserved(&retained);
            } else {
                assert!(
                    !state_dir.join("state.json").exists(),
                    "later observations must occur after an earlier deletion"
                );
            }
            observations.borrow_mut().push(current.to_path_buf());
        })));
    });
    let _observer = ObserverGuard;

    run_clean(
        &directory.path().join(".shipper"),
        directory.path(),
        false,
        false,
        "text",
    )
    .unwrap();

    let observed: BTreeSet<_> = observed.borrow().iter().cloned().collect();
    assert_eq!(observed, directories.into_iter().collect());
}

#[test]
#[serial_test::serial]
fn clean_refuses_held_ownership_even_with_force_or_missing_metadata() {
    for missing_metadata in [false, true] {
        for force in [false, true] {
            let directory = tempdir().unwrap();
            let state_dir = directory.path().join(".shipper");
            let mut retained = retained_artifacts(&state_dir);
            let owner =
                shipper_core::lock::LockFile::acquire(&state_dir, Some(directory.path())).unwrap();
            let lock_path = shipper_core::lock::lock_path(&state_dir, Some(directory.path()));
            if missing_metadata {
                fs::remove_file(&lock_path).unwrap();
            } else {
                let mut record: serde_json::Value =
                    serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
                record["acquired_at"] =
                    serde_json::to_value(Utc::now() - chrono::Duration::days(365)).unwrap();
                let bytes = serde_json::to_vec(&record).unwrap();
                fs::write(&lock_path, &bytes).unwrap();
                retained.push((lock_path.clone(), bytes));
            }
            let error = clean_single_dir(&state_dir, directory.path(), false, force).unwrap_err();
            assert!(format!("{error:#}").contains("OS ownership is active"));
            assert_preserved(&retained);
            assert_eq!(lock_path.exists(), !missing_metadata);
            owner.release().unwrap();
        }
    }
}

#[test]
#[serial_test::serial]
fn clean_without_force_preserves_corrupt_metadata_and_all_artifacts() {
    let directory = tempdir().unwrap();
    let state_dir = directory.path().join(".shipper");
    let mut retained = retained_artifacts(&state_dir);
    let lock_path = shipper_core::lock::lock_path(&state_dir, Some(directory.path()));
    let corrupt = vec![0xff, b'{', b' ', b'\n'];
    fs::write(&lock_path, &corrupt).unwrap();
    retained.push((lock_path.clone(), corrupt));
    assert!(clean_single_dir(&state_dir, directory.path(), false, false).is_err());
    assert_preserved(&retained);
    clean_single_dir(&state_dir, directory.path(), false, true).unwrap();
    assert!(!lock_path.exists());
    for (path, _) in retained {
        assert!(!path.exists());
    }
    let mut guard_path = lock_path.as_os_str().to_os_string();
    guard_path.push(".guard");
    assert!(
        Path::new(&guard_path).exists(),
        "clean leaves the guard inode in place"
    );
}

#[test]
#[serial_test::serial]
fn clean_preserves_cleanup_incomplete_hold_even_with_force_or_corrupt_projection() {
    for source in [
        "event_missing_state",
        "event_corrupt_state",
        "state_corrupt_events",
        "event_before_corrupt_line",
        "event_after_corrupt_line",
    ] {
        for force in [false, true] {
            for keep_receipt in [false, true] {
                let directory = tempdir().unwrap();
                let state_dir = directory.path().join(".shipper");
                let paths = retained_artifacts(&state_dir);
                let state_path = state_dir.join("state.json");
                let event_path = state_dir.join("events.jsonl");
                if source == "state_corrupt_events" {
                    let now = Utc::now();
                    let state = ExecutionState {
                        state_version: "shipper.state.v1".into(),
                        plan_id: "retained-cleanup-plan".into(),
                        registry: Registry::crates_io(),
                        created_at: now,
                        updated_at: now,
                        attempt_history: Vec::new(),
                        packages: BTreeMap::from([(
                            "demo@0.1.0".into(),
                            PackageProgress {
                                name: "demo".into(),
                                version: "0.1.0".into(),
                                attempts: 1,
                                state: PackageState::CleanupIncomplete {
                                    message: "cleanup could not be confirmed".into(),
                                },
                                last_updated_at: now,
                            },
                        )]),
                    };
                    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
                } else {
                    if source == "event_missing_state" {
                        fs::remove_file(&state_path).unwrap();
                    }
                    let event = PublishEvent {
                        timestamp: Utc::now(),
                        package: "demo@0.1.0".into(),
                        event_type: EventType::PackageCleanupIncomplete {
                            message: "cleanup could not be confirmed".into(),
                        },
                    };
                    let raw = format!("{}\n", serde_json::to_string(&event).unwrap());
                    let raw = match source {
                        "event_before_corrupt_line" => format!("{raw}malformed-secret-sentinel\n"),
                        "event_after_corrupt_line" => format!("malformed-secret-sentinel\n{raw}"),
                        _ => raw,
                    };
                    fs::write(&event_path, raw).unwrap();
                }
                let retained: Vec<_> = paths
                    .into_iter()
                    .filter(|(path, _)| path.exists())
                    .map(|(path, _)| {
                        let bytes = fs::read(&path).unwrap();
                        (path, bytes)
                    })
                    .collect();
                let error = clean_single_dir(&state_dir, directory.path(), keep_receipt, force)
                    .unwrap_err();
                assert_eq!(
                    engine::classify_publish_stop(&error),
                    Some(engine::PublishStopClassification::CleanupIncomplete)
                );
                assert!(!format!("{error:#}").contains("malformed-secret-sentinel"));
                assert_preserved(&retained);
                assert_eq!(state_path.exists(), source != "event_missing_state");
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn clean_force_preserves_positively_live_legacy_holder_and_artifacts() {
    let directory = tempdir().unwrap();
    let state_dir = directory.path().join(".shipper");
    let mut retained = retained_artifacts(&state_dir);
    let lock_path = shipper_core::lock::lock_path(&state_dir, Some(directory.path()));
    let owner = shipper_core::lock::LockFile::acquire(&state_dir, Some(directory.path())).unwrap();
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    owner.release().unwrap();
    record.as_object_mut().unwrap().remove("ownership");
    record["acquired_at"] = serde_json::to_value(Utc::now() - chrono::Duration::days(365)).unwrap();
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(&lock_path, &bytes).unwrap();
    retained.push((lock_path, bytes));
    let error = clean_single_dir(&state_dir, directory.path(), false, true).unwrap_err();
    assert!(format!("{error:#}").contains("LiveLegacy"));
    assert_preserved(&retained);
}
