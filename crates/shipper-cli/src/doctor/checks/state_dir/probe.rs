//! Probe only a newly and exclusively created disposable file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_PROBE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::doctor) enum Operation {
    Create,
    Write,
    Remove,
}

impl Operation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Write => "write",
            Self::Remove => "remove",
        }
    }
}

#[derive(Debug)]
pub(super) struct Failure {
    pub operation: Operation,
    pub kind: io::ErrorKind,
    pub cleanup_kind: Option<io::ErrorKind>,
    pub retained_path: Option<PathBuf>,
}

pub(super) fn directory(path: &Path) -> Result<(), Failure> {
    // create_new, not the naming convention, establishes ownership. A name
    // collision is never opened, truncated, or removed.
    for _ in 0..8 {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let serial = NEXT_PROBE.fetch_add(1, Ordering::Relaxed);
        let candidate = path.join(format!(
            ".shipper-doctor-probe-{}-{stamp}-{serial}",
            std::process::id()
        ));
        match candidate_file(&candidate) {
            Err(error)
                if error.operation == Operation::Create
                    && error.kind == io::ErrorKind::AlreadyExists => {}
            result => return result,
        }
    }
    Err(Failure {
        operation: Operation::Create,
        kind: io::ErrorKind::AlreadyExists,
        cleanup_kind: None,
        retained_path: None,
    })
}

fn candidate_file(path: &Path) -> Result<(), Failure> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| Failure {
            operation: Operation::Create,
            kind: error.kind(),
            cleanup_kind: None,
            retained_path: None,
        })?;
    let written = file.write_all(b"shipper doctor disposable access probe\n");
    drop(file);
    // This exact path was created by this call. Never remove a candidate when
    // create_new failed, and never enumerate or clean up another probe name.
    let removed = fs::remove_file(path);
    match (written, removed) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), cleanup) => Err(Failure {
            operation: Operation::Write,
            kind: error.kind(),
            cleanup_kind: cleanup.as_ref().err().map(io::Error::kind),
            retained_path: cleanup.is_err().then(|| path.to_path_buf()),
        }),
        (Ok(()), Err(error)) => Err(Failure {
            operation: Operation::Remove,
            kind: error.kind(),
            cleanup_kind: None,
            retained_path: Some(path.to_path_buf()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn collision_preserves_the_existing_candidate_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".shipper-doctor-probe-existing");
        fs::write(&path, b"existing evidence must survive").unwrap();
        let error = candidate_file(&path).expect_err("create_new must refuse a collision");
        assert_eq!(error.operation, Operation::Create);
        assert_eq!(error.kind, io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(path).unwrap(), b"existing evidence must survive");
    }
}
