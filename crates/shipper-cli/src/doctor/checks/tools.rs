//! External-tool version probes (cargo, git).

use std::process::Command;

use shipper_core::engine::Reporter;

pub(in crate::doctor) fn check(reporter: &mut dyn Reporter) {
    print_cmd_version("cargo", reporter);
    print_cmd_version("git", reporter);
}

pub(crate) fn print_cmd_version(cmd: &str, reporter: &mut dyn Reporter) {
    let out = Command::new(cmd).arg("--version").output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("{cmd}: {s}");
        }
        Ok(o) => {
            reporter.warn(&format!(
                "{cmd} --version failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => {
            reporter.warn(&format!("unable to run {cmd} --version: {e}"));
        }
    }
}
