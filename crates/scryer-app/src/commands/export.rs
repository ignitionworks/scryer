//! The HTML export: one self-contained file a host can hand a user as a
//! download.
//!
//! Upstream already builds exactly this artifact — `scripts/export-html.mjs`
//! bakes a `.scry` into the lifted diagram viewer and collapses the whole Vite
//! build into a single `.html`, fonts and all. That file is the export; this
//! command's job is to run it and hand back the bytes, NOT to grow a second
//! exporter that drifts from the one the CLI ships.
//!
//! So it shells out to the script rather than porting it. The output is a
//! React + Tailwind bundle inlined by Vite: reproducing that in Rust would mean
//! reproducing a bundler, and any port would be a second answer to "what does
//! an export look like" that nobody would keep in step. The cost is an honest
//! one — the service needs `node` and a scryer checkout with its dependencies
//! installed — and it is paid loudly, with a refusal that names what is
//! missing, rather than silently producing a lesser file.
//!
//! The model is staged, not read in place. The layer the host asked for is
//! read through `scryer_core` — which is what makes "the plan" mean the plan
//! even on a project that has no `planned.scry` yet — and written into a
//! throwaway project the script then exports. A concurrent write can't tear
//! the export, and the layer choice is settled here rather than in a flag.

use std::path::{Path, PathBuf};

use crate::error::{CommandError, CommandResult};

/// Which of the two model layers to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// `model.scry` — what the codebase is held to.
    Committed,
    /// `planned.scry` — the canvas's draft, seeded from committed when a
    /// project has never had one.
    Planned,
}

/// How long the bundler gets. A Vite build of the viewer is a few seconds on
/// any machine; a minute and a half means something is wrong, and a host
/// waiting forever is worse than a host told so.
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Build the self-contained HTML export of a project's model and return it.
pub async fn export_html(project_path: &str, layer: Layer) -> CommandResult<String> {
    let r = scryer_core::ModelRef::ProjectLocal(PathBuf::from(project_path));
    let model = match layer {
        Layer::Committed => scryer_core::read_model_raw_at(&r),
        Layer::Planned => scryer_core::read_planned_raw_at(&r),
    }
    .map_err(|e| CommandError::failed(format!("no {} model to export: {e}", name(layer))))?;

    let script = find_export_script().ok_or_else(|| {
        CommandError::failed(
            "no export script: the service needs a scryer checkout with its \
             dependencies installed. Point `SCRYER_EXPORT_SCRIPT` at \
             `scripts/export-html.mjs` in one.",
        )
    })?;
    // The script resolves the viewer and its config from its own location, so
    // running it from the checkout it lives in is the whole of the setup.
    let repo_root = script
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| CommandError::failed("the export script is not inside a checkout"))?;

    // A throwaway project holding just the layer we were asked for, so the
    // script always exports "the committed model" of something consistent.
    let work = tempfile::tempdir()
        .map_err(|e| CommandError::failed(format!("no temporary directory: {e}")))?;
    let staged = work.path().join("project");
    std::fs::create_dir_all(staged.join(".scryer"))
        .map_err(|e| CommandError::failed(format!("could not stage the model: {e}")))?;
    std::fs::write(staged.join(".scryer").join("model.scry"), &model)
        .map_err(|e| CommandError::failed(format!("could not stage the model: {e}")))?;
    let out = work.path().join("scryer-diagram.html");

    let run = tokio::process::Command::new("node")
        .arg(&script)
        .arg(&staged)
        .arg("-o")
        .arg(&out)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .output();
    let output = tokio::time::timeout(EXPORT_TIMEOUT, run)
        .await
        .map_err(|_| {
            CommandError::failed(format!(
                "the export did not finish within {}s",
                EXPORT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| {
            CommandError::failed(format!(
                "could not run the export (is node installed?): {e}"
            ))
        })?;
    if !output.status.success() {
        // The script's own diagnosis, not just its exit code — a missing
        // dependency in the checkout reads as itself.
        return Err(CommandError::failed(format!(
            "the export failed: {}",
            tail(&output.stderr, &output.stdout)
        )));
    }

    std::fs::read_to_string(&out)
        .map_err(|e| CommandError::failed(format!("the export produced no readable file: {e}")))
}

fn name(layer: Layer) -> &'static str {
    match layer {
        Layer::Committed => "committed",
        Layer::Planned => "planned",
    }
}

/// The last few lines the script said before giving up, stderr preferred.
fn tail(stderr: &[u8], stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(if stderr.is_empty() { stdout } else { stderr });
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(10);
    lines[from..].join("\n")
}

/// Find `scripts/export-html.mjs` in a checkout that also carries the viewer it
/// builds.
///
/// A host that installs the service somewhere of its own sets
/// `SCRYER_EXPORT_SCRIPT` and is done. Otherwise: the checkout this binary was
/// built from (right in a dev tree and in the tests, gone in a shipped binary,
/// so it is only ever a candidate), then above the executable, then above the
/// working directory.
fn find_export_script() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("SCRYER_EXPORT_SCRIPT") {
        let path = PathBuf::from(named);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(built_from) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        roots.push(built_from.to_path_buf());
    }
    for start in [std::env::current_exe().ok(), std::env::current_dir().ok()]
        .into_iter()
        .flatten()
    {
        roots.extend(start.ancestors().map(Path::to_path_buf));
    }

    roots.into_iter().find_map(|root| {
        let script = root.join("scripts").join("export-html.mjs");
        // The viewer beside it, or the script would run and fail on its config.
        let viewer = root.join("export-viewer").join("vite.config.ts");
        (script.is_file() && viewer.is_file()).then_some(script)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checkout is found without anything being configured — which is what
    /// makes the command work in a dev tree and in the tests.
    #[test]
    fn the_export_script_is_found_in_its_own_checkout() {
        let script = find_export_script().expect("this crate is inside a scryer checkout");
        assert!(script.ends_with("scripts/export-html.mjs"), "{script:?}");
        assert!(script.is_file());
    }

    /// A host names the layer in the words the model uses for it.
    #[test]
    fn a_layer_is_named_the_way_the_model_names_it() {
        assert_eq!(
            serde_json::from_value::<Layer>(serde_json::json!("committed")).unwrap(),
            Layer::Committed
        );
        assert_eq!(
            serde_json::from_value::<Layer>(serde_json::json!("planned")).unwrap(),
            Layer::Planned
        );
        let err = serde_json::from_value::<Layer>(serde_json::json!("draft")).unwrap_err();
        assert!(err.to_string().contains("committed"), "{err}");
    }

    /// A project with no model of the asked-for layer refuses before it spends
    /// a bundler run finding out.
    #[tokio::test]
    async fn a_project_with_no_model_refuses_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let err = export_html(&dir.path().to_string_lossy(), Layer::Committed)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no committed model to export"),
            "{err}"
        );
        assert_eq!(err.status(), 500);
    }
}
