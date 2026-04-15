//! path_edge_case_tests for the `state::store` module.
//! Absorbed from former `shipper-store` crate.

use super::*;
use crate::types::{PackageProgress, PackageState, Registry};
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::tempdir;

fn sample_state() -> ExecutionState {
    let mut packages = BTreeMap::new();
    packages.insert(
        "demo@0.1.0".to_string(),
        PackageProgress {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state: PackageState::Pending,
            last_updated_at: Utc::now(),
        },
    );
    ExecutionState {
        state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
        plan_id: "p1".to_string(),
        registry: Registry::crates_io(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        packages,
    }
}

// ── Path with spaces ────────────────────────────────────────────

#[test]
fn file_store_path_with_spaces() {
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("my state dir");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = FileStore::new(dir.clone());

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load");
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().plan_id, "p1");
}

// ── Path with Unicode characters ────────────────────────────────

#[test]
fn file_store_path_with_unicode() {
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("日本語パス");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = FileStore::new(dir);

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load");
    assert!(loaded.is_some());
}

// ── Path with emoji ─────────────────────────────────────────────

#[test]
fn file_store_path_with_emoji() {
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("📦release");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = FileStore::new(dir);

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load");
    assert!(loaded.is_some());
}

// ── Deeply nested path ──────────────────────────────────────────

#[test]
fn file_store_deeply_nested_path() {
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("a").join("b").join("c").join("d").join("e");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = FileStore::new(dir);

    store.save_state(&sample_state()).expect("save");
    let loaded = store.load_state().expect("load");
    assert!(loaded.is_some());
}

// ── Load from non-existent directory returns None ────────────────

#[test]
fn file_store_load_nonexistent_dir_returns_none() {
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("does_not_exist");
    let store = FileStore::new(dir);

    let loaded = store.load_state().expect("load");
    assert!(loaded.is_none());
}

// ── Clear on empty directory succeeds ────────────────────────────

#[test]
fn file_store_clear_empty_dir_succeeds() {
    let td = tempdir().expect("tempdir");
    let store = FileStore::new(td.path().to_path_buf());
    store.clear().expect("clear on empty dir should succeed");
}

// ── state_dir accessor returns correct path ─────────────────────

#[test]
fn file_store_state_dir_accessor() {
    let dir = PathBuf::from("some/path with spaces/dir");
    let store = FileStore::new(dir.clone());
    assert_eq!(store.state_dir(), &dir);
}
