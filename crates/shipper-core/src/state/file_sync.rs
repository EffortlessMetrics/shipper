//! The private file-sync seam shared by event and JSON persistence.
//!
//! Fault observation exists only in unit-test builds and is scoped to one
//! temporary directory. Tests can fail before an event append or after a
//! caller has written and flushed a file, just before syncing it.

use std::fs::File;
use std::io;
use std::path::Path;

pub(super) fn sync_all(file: &File, _path: &Path) -> io::Result<()> {
    #[cfg(test)]
    test_support::before_io(test_support::Point::Sync, _path)?;
    file.sync_all()
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    type Observer = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(in crate::state) enum Point {
        Sync,
        EventAppend,
    }

    static OBSERVER: Mutex<Option<(PathBuf, Point, Observer)>> = Mutex::new(None);

    pub(crate) struct Guard;

    /// Serial tests install one observer for their own absolute temporary root.
    /// Other tests and worker threads writing elsewhere are unaffected.
    pub(crate) fn observe(
        root: &Path,
        observer: impl Fn(&Path) -> io::Result<()> + Send + Sync + 'static,
    ) -> Guard {
        observe_at(root, Point::Sync, observer)
    }

    /// Fail an event append before any bytes from its batch reach the file.
    pub(crate) fn observe_event_append(
        root: &Path,
        observer: impl Fn(&Path) -> io::Result<()> + Send + Sync + 'static,
    ) -> Guard {
        observe_at(root, Point::EventAppend, observer)
    }

    fn observe_at(
        root: &Path,
        point: Point,
        observer: impl Fn(&Path) -> io::Result<()> + Send + Sync + 'static,
    ) -> Guard {
        assert!(root.is_absolute());
        let mut active = OBSERVER.lock().expect("file-sync observer mutex");
        assert!(
            active.is_none(),
            "persistence fault tests must run serially"
        );
        *active = Some((root.to_path_buf(), point, Arc::new(observer)));
        Guard
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            *OBSERVER.lock().expect("file-sync observer mutex") = None;
        }
    }

    pub(in crate::state) fn before_io(point: Point, path: &Path) -> io::Result<()> {
        let observer = OBSERVER
            .lock()
            .expect("file-sync observer mutex")
            .as_ref()
            .filter(|(root, observed, _)| *observed == point && path.starts_with(root))
            .map(|(_, _, observer)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer(path)?;
        }
        Ok(())
    }
}
