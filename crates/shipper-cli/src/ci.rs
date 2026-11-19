//! GitHub Actions step output. Other providers retain their existing recipes.

use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};

/// The generated job runs at the checkout/workspace root. A literal relative
/// path has the same meaning to Shipper, Bash and the artifact action.
fn github_state_path(state_dir: &Path) -> Result<String> {
    let raw = state_dir
        .to_str()
        .context("GitHub Actions state paths must be valid UTF-8")?;
    ensure!(
        !raw.chars().any(char::is_control)
            && !raw.contains(['\\', ':', '*', '?', '[', ']', '!'])
            && !raw.contains("${{"),
        "GitHub Actions requires a literal relative --state-dir with forward slashes; control characters, drive prefixes, globs and workflow expressions are unsupported"
    );
    let mut components = Vec::new();
    for component in state_dir.components() {
        match component {
            Component::Normal(value) => components.push(
                value
                    .to_str()
                    .context("GitHub Actions state paths must be valid UTF-8")?,
            ),
            Component::CurDir => {}
            _ => anyhow::bail!(
                "GitHub Actions requires a checkout-relative --state-dir without parent traversal; absolute paths from the generator's machine are not portable"
            ),
        }
    }
    ensure!(
        !components.is_empty(),
        "GitHub Actions --state-dir must name a directory below the checkout root"
    );
    // In particular, ./ prevents a leading ~ from becoming an artifact-tool
    // home-directory expansion. Shell expansion cannot reinterpret the env value.
    Ok(format!("./{}", components.join("/")))
}

pub(super) fn github_actions_steps(state_dir: &Path) -> Result<String> {
    let path = github_state_path(state_dir)?;
    let shell_path = serde_json::to_string(&path)?;
    let artifact_path = serde_json::to_string(&format!("{path}/"))?;
    Ok(format!(
        r#"# GitHub Actions steps for a fresh, approved Shipper publication.
# Insert under jobs.publish.steps; this is not a complete workflow file.
# The Cargo workspace must be at the checkout root. Install Shipper first.
# Use preflight --preflight-only; select retained-artifact recovery separately.
- name: Publish crates from fresh evidence
  shell: bash
  working-directory: ${{{{ github.workspace }}}}
  env:
    SHIPPER_STATE_DIR: {shell_path}
    CARGO_REGISTRY_TOKEN: ${{{{ secrets.CARGO_REGISTRY_TOKEN }}}}
  run: |
    if [ -e "$SHIPPER_STATE_DIR" ] || [ -L "$SHIPPER_STATE_DIR" ]; then
      retained=$(find -L "$SHIPPER_STATE_DIR" \( -name state.json -o -name events.jsonl -o -name receipt.json -o -name reconciliation.json \) -print -quit)
      if [ -n "$retained" ]; then
        printf '%s\n' 'Retained release evidence is present. Inspect the selected source artifact and choose an approved recovery operation before publishing.' >&2
        exit 1
      fi
    fi
    shipper publish --quiet --state-dir "$SHIPPER_STATE_DIR"

- name: Retain Shipper evidence
  if: always()
  uses: actions/upload-artifact@v7
  with:
    name: shipper-state-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}
    path: {artifact_path}
    include-hidden-files: true
    if-no-files-found: error
"#
    ))
}
