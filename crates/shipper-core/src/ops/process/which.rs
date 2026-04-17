//! Locate executables on `PATH`.

/// Check if a command exists in PATH.
#[allow(dead_code)]
pub(crate) fn command_exists(program: &str) -> bool {
    which::which(program).is_ok()
}

/// Get the full path to a command.
#[allow(dead_code)]
pub(crate) fn which(program: &str) -> Option<std::path::PathBuf> {
    which::which(program).ok()
}
