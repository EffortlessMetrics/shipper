//! External-tool version probes (cargo, git).

use std::process::Command;

use serde::Serialize;
use shipper_core::engine::Reporter;

pub(in crate::doctor) fn check(reporter: &mut dyn Reporter) {
    for check in inspect() {
        print_tool_check(&check, reporter);
    }
}

#[derive(Debug, Serialize)]
pub(in crate::doctor) struct ToolCheck {
    pub command: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(in crate::doctor) fn inspect() -> Vec<ToolCheck> {
    vec![cmd_version("cargo"), cmd_version("git")]
}

#[cfg(test)]
pub(crate) fn print_cmd_version(cmd: &str, reporter: &mut dyn Reporter) {
    let check = cmd_version(cmd);
    print_tool_check(&check, reporter);
}

fn print_tool_check(check: &ToolCheck, reporter: &mut dyn Reporter) {
    if let Some(version) = &check.version {
        println!("{}: {}", check.command, version);
    } else if let Some(error) = &check.error {
        reporter.warn(error);
    }
}

fn cmd_version(cmd: &str) -> ToolCheck {
    let out = Command::new(cmd).arg("--version").output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            ToolCheck {
                command: command_name(cmd),
                version: Some(s),
                error: None,
            }
        }
        Ok(o) => ToolCheck {
            command: command_name(cmd),
            version: None,
            error: Some(format!(
                "{cmd} --version failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        },
        Err(e) => ToolCheck {
            command: command_name(cmd),
            version: None,
            error: Some(format!("unable to run {cmd} --version: {e}")),
        },
    }
}

fn command_name(cmd: &str) -> &'static str {
    match cmd {
        "cargo" => "cargo",
        "git" => "git",
        _ => "unknown",
    }
}
