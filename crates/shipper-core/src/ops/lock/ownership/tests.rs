use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use chrono::TimeZone;
use tempfile::tempdir;

use super::*;

fn bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read retained bytes")
}

fn duplicate_ownership(record: &serde_json::Value) -> Vec<u8> {
    let mut base = record.clone();
    base.as_object_mut().unwrap().remove("ownership");
    let mut json = serde_json::to_string(&base).unwrap();
    assert_eq!(json.pop(), Some('}'));
    json.push_str(r#", "ownership": {"protocol": 99}, "ownership": {"owner": false}}"#);
    json.into_bytes()
}

#[test]
#[serial_test::serial]
fn released_handle_cannot_mutate_or_release_its_successor() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let first = LockFile::acquire(directory.path(), None).unwrap();
    first.set_plan_id("first").unwrap();
    first.release().unwrap();
    let second = LockFile::acquire(directory.path(), None).unwrap();
    second.set_plan_id("second").unwrap();
    let retained = bytes(&path);

    assert!(first.set_plan_id("stale-first").is_err());
    assert!(first.read_owned_record().is_err());
    first.release().unwrap();
    drop(first);
    assert_eq!(bytes(&path), retained);
    assert_eq!(
        second.read_owned_record().unwrap().info.plan_id.as_deref(),
        Some("second")
    );
    for force in [false, true] {
        assert!(
            LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, force)
                .is_err()
        );
        assert_eq!(bytes(&path), retained);
    }
    second.release().unwrap();
    let third = LockFile::acquire(directory.path(), None).unwrap();
    assert!(guard_path(&path).exists(), "sidecar is never unlinked");
    drop(third);
    assert!(guard_path(&path).exists());
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn inherited_descriptor_does_not_extend_released_or_dropped_owner() {
    for explicit_release in [false, true] {
        let directory = tempdir().unwrap();
        let first = LockFile::acquire(directory.path(), None).unwrap();
        // Test-only duplication models a forked child's brief pre-exec
        // inheritance. Production never clones its OsGuard or exposes File.
        let inherited = first
            .owned
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .guard
            .file
            .as_ref()
            .unwrap()
            .try_clone()
            .unwrap();
        if explicit_release {
            first.release().unwrap();
        }
        drop(first);
        let second = LockFile::acquire(directory.path(), None).unwrap();
        second.set_plan_id("successor").unwrap();
        let path = lock_path(directory.path(), None);
        let retained = bytes(&path);
        drop(inherited);
        assert_eq!(bytes(&path), retained);
        assert!(
            LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, true)
                .is_err()
        );
        second.release().unwrap();
        drop(LockFile::acquire(directory.path(), None).unwrap());
    }
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn inherited_descriptor_does_not_extend_dropped_unclaimed_reservation() {
    let directory = tempdir().unwrap();
    let reservation =
        LockReservation::acquire(directory.path(), None, Duration::ZERO, false).unwrap();
    let inherited = reservation
        .guard
        .file
        .as_ref()
        .unwrap()
        .try_clone()
        .unwrap();
    drop(reservation);
    let successor = LockFile::acquire(directory.path(), None).unwrap();
    let path = lock_path(directory.path(), None);
    let retained = bytes(&path);
    drop(inherited);
    assert_eq!(bytes(&path), retained);
    assert!(LockFile::acquire(directory.path(), None).is_err());
    successor.release().unwrap();
    drop(LockFile::acquire(directory.path(), None).unwrap());
}

#[test]
#[serial_test::serial]
fn owner_refuses_replaced_metadata_and_preserves_it_on_release() {
    let directory = tempdir().unwrap();
    let first = LockFile::acquire(directory.path(), None).unwrap();
    let path = lock_path(directory.path(), None);
    let mut foreign: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    foreign["ownership"]["owner"] = serde_json::Value::String("f".repeat(32));
    foreign["plan_id"] = serde_json::Value::String("foreign-record".into());
    let retained = serde_json::to_vec_pretty(&foreign).unwrap();
    fs::write(&path, &retained).unwrap();
    assert!(first.set_plan_id("stale").is_err());
    assert!(first.read_owned_record().is_err());
    first.release().unwrap();
    drop(first);
    assert_eq!(bytes(&path), retained);
}

#[test]
#[serial_test::serial]
fn unclaimed_reservation_preserves_predecessor_bytes_and_excludes_other_owners() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let retained = b" {\n  retained-but-corrupt predecessor\n } \n";
    fs::write(&path, retained).unwrap();
    let reservation =
        LockReservation::acquire(directory.path(), None, Duration::ZERO, true).unwrap();
    assert!(matches!(
        reservation.prior_contents(),
        LockContents::Corrupt
    ));
    assert_eq!(bytes(&path), retained);
    assert!(
        LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, true)
            .is_err()
    );
    drop(reservation);
    assert_eq!(bytes(&path), retained);
    assert!(LockFile::acquire_with_timeout(directory.path(), None, Duration::ZERO).is_err());

    let reservation =
        LockReservation::acquire(directory.path(), None, Duration::ZERO, true).unwrap();
    let claimed = reservation.claim(Some("admitted-plan")).unwrap();
    assert_eq!(
        claimed.read_owned_record().unwrap().info.plan_id.as_deref(),
        Some("admitted-plan")
    );
    drop(claimed);
    assert!(!path.exists());
    assert!(guard_path(&path).exists());
}

#[test]
#[serial_test::serial]
fn missing_metadata_reservation_still_excludes_claimers_and_force() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let reservation =
        LockReservation::acquire(directory.path(), None, Duration::ZERO, false).unwrap();
    assert!(matches!(
        reservation.prior_contents(),
        LockContents::Missing
    ));
    assert!(!path.exists());
    assert!(LockFile::acquire(directory.path(), None).is_err());
    assert!(
        LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, true)
            .is_err()
    );
    drop(reservation);
    assert!(!path.exists());
    drop(LockFile::acquire(directory.path(), None).unwrap());
}

#[test]
#[serial_test::serial]
fn live_os_ownership_ignores_old_and_future_metadata_even_with_force() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    for offset in [-365, 365] {
        let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
        record["acquired_at"] =
            serde_json::to_value(Utc::now() + chrono::Duration::days(offset)).unwrap();
        let retained = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &retained).unwrap();
        for force in [false, true] {
            assert!(
                LockFile::acquire_with_timeout_and_force(
                    directory.path(),
                    None,
                    Duration::ZERO,
                    force
                )
                .is_err()
            );
            assert_eq!(bytes(&path), retained);
        }
    }
    owner.release().unwrap();
    assert!(guard_path(&path).exists());
}

#[test]
#[serial_test::serial]
fn known_protocol_orphan_is_recoverable_without_relying_on_age_or_pid() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    for offset in [-365, 365] {
        let owner = LockFile::acquire(directory.path(), None).unwrap();
        let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
        owner.release().unwrap();
        record["acquired_at"] =
            serde_json::to_value(Utc::now() + chrono::Duration::days(offset)).unwrap();
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            LockFile::acquire(directory.path(), None).is_err(),
            "strict adapter preserves retained metadata"
        );
        let reservation =
            LockReservation::acquire(directory.path(), None, Duration::MAX, false).unwrap();
        assert!(matches!(
            reservation.prior_contents(),
            LockContents::Present(_)
        ));
        let successor = reservation.claim(None).unwrap();
        assert_ne!(
            serde_json::from_slice::<serde_json::Value>(&bytes(&path)).unwrap()["ownership"],
            record["ownership"]
        );
        drop(owner);
        assert!(LockFile::acquire(directory.path(), None).is_err());
        drop(successor);
    }
}

#[test]
#[serial_test::serial]
fn unknown_metadata_requires_explicit_force_and_is_preserved_on_refusal() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    let mut foreign = original.clone();
    foreign["hostname"] = serde_json::Value::String(format!("{}-foreign", local_hostname()));
    let mut unknown_protocol = original.clone();
    unknown_protocol["ownership"]["protocol"] = serde_json::Value::String("future-protocol".into());
    unknown_protocol
        .as_object_mut()
        .unwrap()
        .remove("process_identity");
    let mut legacy = original;
    legacy.as_object_mut().unwrap().remove("ownership");
    legacy.as_object_mut().unwrap().remove("process_identity");
    for retained in [
        Vec::new(),
        b"{truncated".to_vec(),
        vec![0xff, 0xfe],
        serde_json::to_vec(&foreign).unwrap(),
        serde_json::to_vec(&unknown_protocol).unwrap(),
        serde_json::to_vec(&legacy).unwrap(),
    ] {
        fs::write(&path, &retained).unwrap();
        assert!(LockFile::acquire_with_timeout(directory.path(), None, Duration::ZERO).is_err());
        assert_eq!(bytes(&path), retained);
        let reservation =
            LockReservation::acquire(directory.path(), None, Duration::ZERO, true).unwrap();
        assert_eq!(bytes(&path), retained);
        drop(reservation);
        assert_eq!(bytes(&path), retained);
        drop(
            LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, true)
                .unwrap(),
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn positively_live_legacy_holder_is_not_overridden_even_by_force() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    record.as_object_mut().unwrap().remove("ownership");
    for offset in [-365, 365] {
        record["acquired_at"] =
            serde_json::to_value(Utc::now() + chrono::Duration::days(offset)).unwrap();
        let retained = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &retained).unwrap();
        for force in [false, true] {
            assert!(
                LockFile::acquire_with_timeout_and_force(
                    directory.path(),
                    None,
                    Duration::ZERO,
                    force
                )
                .is_err()
            );
            assert_eq!(bytes(&path), retained);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn malformed_ownership_extension_cannot_hide_a_positively_live_legacy_holder() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    record["acquired_at"] = serde_json::to_value(Utc::now() - chrono::Duration::days(365)).unwrap();
    let mut encodings = vec![duplicate_ownership(&record)];
    for extension in [
        serde_json::json!({"protocol": 99}),
        serde_json::json!({"protocol": OWNERSHIP_PROTOCOL, "owner": 99}),
        serde_json::json!("future ownership encoding"),
        serde_json::Value::Null,
    ] {
        record["ownership"] = extension;
        encodings.push(serde_json::to_vec(&record).unwrap());
    }
    for retained in encodings {
        fs::write(&path, &retained).unwrap();
        // The unsafe historical behavior must fail on permission to claim,
        // before a diagnostic-string assertion can stop the negative control.
        for force in [true, false] {
            let error = LockReservation::acquire(directory.path(), None, Duration::ZERO, force)
                .unwrap_err();
            assert!(error.to_string().contains("LiveLegacy"), "{error:#}");
            assert_eq!(bytes(&path), retained);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn malformed_ownership_extension_with_absent_or_unknown_holder_requires_force() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    for known_absent in [false, true] {
        let mut record = original.clone();
        record["pid"] = serde_json::Value::from(u32::MAX);
        record["acquired_at"] =
            serde_json::to_value(Utc::now() - chrono::Duration::days(365)).unwrap();
        record["ownership"] = serde_json::json!({"protocol": 99});
        if !known_absent {
            record.as_object_mut().unwrap().remove("process_identity");
        }
        for retained in [
            serde_json::to_vec(&record).unwrap(),
            duplicate_ownership(&record),
        ] {
            fs::write(&path, &retained).unwrap();
            assert!(
                LockReservation::acquire(directory.path(), None, Duration::ZERO, false).is_err()
            );
            assert_eq!(bytes(&path), retained);
            let reservation =
                LockReservation::acquire(directory.path(), None, Duration::ZERO, true).unwrap();
            assert!(
                matches!(reservation.prior_contents(), LockContents::Present(_)),
                "additive fields must not erase a readable base identity"
            );
            assert_eq!(bytes(&path), retained);
            drop(reservation);
            assert_eq!(bytes(&path), retained);
            drop(
                LockFile::acquire_with_timeout_and_force(
                    directory.path(),
                    None,
                    Duration::ZERO,
                    true,
                )
                .unwrap(),
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn duplicate_ownership_keeps_unknown_base_evidence_readable_on_every_platform() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    record.as_object_mut().unwrap().remove("process_identity");
    let retained = duplicate_ownership(&record);
    fs::write(&path, &retained).unwrap();
    let reservation =
        LockReservation::acquire(directory.path(), None, Duration::ZERO, true).unwrap();
    let LockContents::Present(base) = reservation.prior_contents() else {
        panic!("duplicate additive extension discarded the base evidence")
    };
    assert_eq!(base.info.pid, std::process::id());
    assert!(base.process_identity.is_none());
    drop(reservation);
    assert_eq!(bytes(&path), retained);
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn legacy_recovery_requires_local_scope_and_positive_absence() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let owner = LockFile::acquire(directory.path(), None).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&bytes(&path)).unwrap();
    owner.release().unwrap();
    record.as_object_mut().unwrap().remove("ownership");
    record["pid"] = serde_json::Value::from(u32::MAX);
    record["acquired_at"] = serde_json::to_value(Utc::now() - chrono::Duration::hours(2)).unwrap();
    let mut foreign_scope = record.clone();
    foreign_scope["process_identity"]["pid_namespace"] =
        serde_json::Value::String("different-scope".into());
    let retained = serde_json::to_vec(&foreign_scope).unwrap();
    fs::write(&path, &retained).unwrap();
    assert!(LockFile::acquire_with_timeout(directory.path(), None, Duration::ZERO).is_err());
    assert_eq!(bytes(&path), retained);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    drop(LockFile::acquire_with_timeout(directory.path(), None, Duration::from_hours(1)).unwrap());
}

#[test]
#[serial_test::serial]
fn legacy_timeout_boundary_uses_nonnegative_age_strictly_greater_than_timeout() {
    let now = Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap();
    let timeout = Duration::from_mins(1);
    for (offset, expected) in [
        (-61, true),
        (-60, false),
        (-59, false),
        (0, false),
        (61, false),
    ] {
        let acquired = now + chrono::Duration::seconds(offset);
        assert_eq!(
            recovery_allowed(Predecessor::AbsentLegacy, acquired, now, timeout, false),
            expected
        );
        assert!(!recovery_allowed(
            Predecessor::LiveLegacy,
            acquired,
            now,
            Duration::ZERO,
            true
        ));
        assert!(!recovery_allowed(
            Predecessor::Unknown,
            acquired,
            now,
            Duration::ZERO,
            false
        ));
        assert!(recovery_allowed(
            Predecessor::Unknown,
            acquired,
            now,
            timeout,
            true
        ));
    }
    assert!(!recovery_allowed(
        Predecessor::AbsentLegacy,
        now - chrono::Duration::days(365),
        now,
        Duration::MAX,
        false
    ));
}

#[test]
#[serial_test::serial]
fn guard_io_failure_never_authorizes_metadata_replacement() {
    let directory = tempdir().unwrap();
    let path = lock_path(directory.path(), None);
    let retained = b"unknown predecessor";
    fs::write(&path, retained).unwrap();
    fs::create_dir(guard_path(&path)).unwrap();
    for force in [false, true] {
        assert!(
            LockFile::acquire_with_timeout_and_force(directory.path(), None, Duration::ZERO, force)
                .is_err()
        );
        assert_eq!(bytes(&path), retained);
    }
}

#[test]
#[serial_test::serial]
fn simultaneous_plan_updates_and_release_leave_no_stale_capability() {
    let directory = tempdir().unwrap();
    let first = Arc::new(LockFile::acquire(directory.path(), None).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let writer = {
        let first = Arc::clone(&first);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            first.set_plan_id("before-release")
        })
    };
    let releaser = {
        let first = Arc::clone(&first);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            first.release()
        })
    };
    barrier.wait();
    let _ = writer.join().unwrap(); // Either serialized ordering is valid.
    releaser.join().unwrap().unwrap();
    let second = LockFile::acquire(directory.path(), None).unwrap();
    let retained = bytes(&lock_path(directory.path(), None));
    assert!(first.set_plan_id("after-release").is_err());
    first.release().unwrap();
    drop(first);
    assert_eq!(bytes(&lock_path(directory.path(), None)), retained);
    drop(second);
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "child rendezvous timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_contender(directory: &Path, id: usize) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ops::lock::ownership::tests::lock_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("SHIPPER_LOCK_CHILD_DIRECTORY", directory)
        .env("SHIPPER_LOCK_CHILD_ID", id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

#[test]
#[serial_test::serial]
#[ignore = "helper process for deterministic cross-process ownership tests"]
fn lock_process_child() {
    let Some(directory) = std::env::var_os("SHIPPER_LOCK_CHILD_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let id = std::env::var("SHIPPER_LOCK_CHILD_ID").unwrap();
    fs::write(directory.join(format!("ready-{id}")), b"ready").unwrap();
    wait_until(|| directory.join("go").exists());
    let result = LockFile::acquire(&directory.join("state"), None);
    let temporary = directory.join(format!("result-{id}.tmp"));
    fs::write(&temporary, if result.is_ok() { "owned" } else { "refused" }).unwrap();
    fs::rename(&temporary, directory.join(format!("result-{id}"))).unwrap();
    wait_until(|| directory.join("release").exists());
    drop(result);
}

#[test]
#[serial_test::serial]
fn process_rendezvous_has_exactly_one_owner_held_until_all_contenders_finish() {
    let directory = tempdir().unwrap();
    let count = 6;
    let mut children = Children(
        (0..count)
            .map(|id| spawn_contender(directory.path(), id))
            .collect(),
    );
    wait_until(|| (0..count).all(|id| directory.path().join(format!("ready-{id}")).exists()));
    fs::write(directory.path().join("go"), b"go").unwrap();
    wait_until(|| (0..count).all(|id| directory.path().join(format!("result-{id}")).exists()));
    let winners = (0..count)
        .filter(|id| bytes(&directory.path().join(format!("result-{id}"))) == b"owned")
        .count();
    assert_eq!(
        winners, 1,
        "all children retain their result until the release rendezvous"
    );
    let state_dir = directory.path().join("state");
    let retained = bytes(&lock_path(&state_dir, None));
    assert!(
        LockFile::acquire_with_timeout_and_force(&state_dir, None, Duration::ZERO, true).is_err()
    );
    assert_eq!(bytes(&lock_path(&state_dir, None)), retained);
    fs::write(directory.path().join("release"), b"release").unwrap();
    for child in &mut children.0 {
        assert!(child.wait().unwrap().success());
    }
    drop(LockFile::acquire(&state_dir, None).unwrap());
}

#[test]
#[serial_test::serial]
fn process_abandonment_releases_os_ownership_but_preserves_recoverable_metadata() {
    let directory = tempdir().unwrap();
    let mut children = Children(vec![spawn_contender(directory.path(), 0)]);
    wait_until(|| directory.path().join("ready-0").exists());
    fs::write(directory.path().join("go"), b"go").unwrap();
    wait_until(|| directory.path().join("result-0").exists());
    assert_eq!(bytes(&directory.path().join("result-0")), b"owned");
    let state_dir = directory.path().join("state");
    let path = lock_path(&state_dir, None);
    let retained = bytes(&path);
    children.0[0].kill().unwrap();
    let _ = children.0[0].wait().unwrap();
    assert_eq!(
        bytes(&path),
        retained,
        "process termination does not run Rust Drop"
    );
    assert!(LockFile::acquire(&state_dir, None).is_err());
    let reservation = LockReservation::acquire(&state_dir, None, Duration::MAX, false).unwrap();
    assert_eq!(bytes(&path), retained);
    drop(reservation);
    assert_eq!(bytes(&path), retained);
    drop(LockFile::acquire_with_timeout(&state_dir, None, Duration::MAX).unwrap());
    assert!(guard_path(&path).exists());
}
