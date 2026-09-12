//! The HTML export: one self-contained file a host can hand a user as a
//! download.
//!
//! Upstream builds this artifact from a CLI script — `scripts/export-html.mjs`
//! bakes a `.scry` into the lifted diagram viewer and Vite collapses the whole
//! build into a single `.html`. That is the file a host wants, but the way it
//! is made does not survive leaving the repo: a service consuming `scryer-app`
//! as a library gets the script (it is in the crate's git checkout) and none of
//! its node dependencies (`node_modules` is not), so running it fails on an
//! import before it renders a byte.
//!
//! So the viewer is BUILT ONCE, against a sentinel in place of a model, and the
//! result ships inside this crate. Exporting is then a substitution: find the
//! sentinel, put the project's model in its place. No node, no checkout, no
//! bundler — a few milliseconds instead of a few seconds, anywhere the engine
//! runs. That is sound because the export is structurally a template plus one
//! value: two exports of different models are byte-identical outside a single
//! string literal, which is the only thing `export-viewer/vite.config.ts`
//! derives from the model.
//!
//! The cost is that the artifact is GENERATED and checked in, so it can go
//! stale silently — the export would keep working and keep shipping an old
//! viewer. `cargo run -p xtask -- export-template` regenerates it and
//! `--check` fails when the committed copy is not what a rebuild produces.
//!
//! `SCRYER_EXPORT_SCRIPT` still runs the real script, for developing the viewer
//! without regenerating the artifact. Everything that can go wrong on that path
//! is refused with what was found and what was missing, never a stack trace.
//!
//! The model is staged, not exported in place: the layer the host asked for is
//! read through `scryer_core` — which is what makes "the plan" mean the plan
//! even on a project that has no `planned.scry` yet — so a concurrent write
//! cannot tear the export and the layer choice is settled here.

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

/// The token the shipped viewer carries in place of a project's model. Written
/// by `xtask export-template`, which bakes a model file whose entire contents
/// are this token — so replacing it replaces the model whole rather than
/// patching something around it. Must match `xtask`'s `SENTINEL`.
pub const SENTINEL: &str = "@@SCRYER_MODEL_SENTINEL@@";

/// The viewer, built once and shipped with this crate. ~630 KB of inlined
/// bundle; regenerate with `cargo run -p xtask -- export-template`.
const TEMPLATE: &str = include_str!("../../assets/export-viewer.html");

/// How long the bundler gets on the `SCRYER_EXPORT_SCRIPT` path. A Vite build
/// of the viewer is a few seconds on any machine; a minute and a half means
/// something is wrong, and a host waiting forever is worse than a host told so.
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Build the self-contained HTML export of a project's model and return it.
pub async fn export_html(project_path: &str, layer: Layer) -> CommandResult<String> {
    let r = scryer_core::ModelRef::ProjectLocal(PathBuf::from(project_path));
    let model = match layer {
        Layer::Committed => scryer_core::read_model_raw_at(&r),
        Layer::Planned => scryer_core::read_planned_raw_at(&r),
    }
    .map_err(|e| CommandError::failed(format!("no {} model to export: {e}", name(layer))))?;

    match configured_script()? {
        Some(script) => run_script(&script, &model).await,
        None => render(TEMPLATE, &model),
    }
}

// --- the shipped viewer ------------------------------------------------------

/// Where the model sits in the built viewer, and therefore how it has to be
/// escaped. The bundler emits the baked-in value as ONE JS string literal and
/// picks the quoting itself — a plain double-quoted string for the short
/// sentinel, a template literal when newlines make that shorter. Both are
/// handled; anything else refuses rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Quoted,
    Template,
}

/// Put `model` where the sentinel is.
///
/// Refuses loudly unless the sentinel appears EXACTLY once, in exactly one of
/// the two quotings. A partial or repeated match would produce a file that
/// looks fine and is broken, which is the one outcome worth failing for.
fn render(template: &str, model: &str) -> CommandResult<String> {
    let quoted = format!("\"{SENTINEL}\"");
    let templated = format!("`{SENTINEL}`");
    let (quoted_hits, template_hits) = (
        template.matches(quoted.as_str()).count(),
        template.matches(templated.as_str()).count(),
    );

    let slot = match (quoted_hits, template_hits) {
        (1, 0) => Slot::Quoted,
        (0, 1) => Slot::Template,
        _ => {
            return Err(CommandError::failed(format!(
                "the built-in export viewer does not carry its model slot as expected \
                 ({quoted_hits} quoted, {template_hits} templated occurrence(s) of {SENTINEL}, \
                 wanted exactly one of either). The artifact is stale or the bundler's output \
                 shape changed: regenerate it with `cargo run -p xtask -- export-template`."
            )))
        }
    };

    Ok(match slot {
        // A JSON string literal IS a JS string literal, and serde escapes every
        // quote, backslash and control character for us.
        Slot::Quoted => template.replace(
            quoted.as_str(),
            &serde_json::to_string(model).map_err(|e| CommandError::failed(e.to_string()))?,
        ),
        Slot::Template => template.replace(
            templated.as_str(),
            &format!("`{}`", escape_template_literal(model)),
        ),
    })
}

/// Escape for a JS template literal: the backslash first (or it would double
/// the escapes the other two add), then the backtick that would close it, then
/// the `${` that would open a substitution.
fn escape_template_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

// --- the script, when a developer asks for it --------------------------------

/// The script `SCRYER_EXPORT_SCRIPT` names, checked to the point where running
/// it can actually work. `None` means nobody asked for it — use the shipped
/// viewer.
///
/// Every refusal names the path that WAS found and the thing that was missing,
/// because the failure this replaces was node's own module-resolution stack
/// trace out of a checkout the caller never chose.
fn configured_script() -> CommandResult<Option<PathBuf>> {
    script_from(std::env::var_os("SCRYER_EXPORT_SCRIPT"))
}

/// The checks themselves, with the environment lifted out so each refusal can
/// be exercised directly.
fn script_from(named: Option<std::ffi::OsString>) -> CommandResult<Option<PathBuf>> {
    let Some(named) = named else {
        return Ok(None);
    };
    let script = PathBuf::from(&named);
    let shown = script.display().to_string();
    if !script.is_file() {
        return Err(missing(&shown, "there is no file there"));
    }
    let Some(root) = script.parent().and_then(Path::parent) else {
        return Err(missing(&shown, "it is not inside a checkout"));
    };
    // The viewer it builds…
    let viewer = root.join("export-viewer").join("vite.config.ts");
    if !viewer.is_file() {
        return Err(missing(&shown, &format!("{} is missing", viewer.display())));
    }
    // …and the dependencies it builds the viewer WITH. This is the one a cargo
    // git checkout fails: it carries the whole repo, and `node_modules` is not
    // in the repo.
    let vite = root.join("node_modules").join("vite");
    if !vite.exists() {
        return Err(missing(
            &shown,
            &format!(
                "{} is missing — that checkout has no installed dependencies, so run \
                 `pnpm install` there",
                vite.display()
            ),
        ));
    }
    Ok(Some(script))
}

fn missing(found: &str, problem: &str) -> CommandError {
    CommandError::failed(format!(
        "cannot export with the script SCRYER_EXPORT_SCRIPT names: found {found}, but {problem}. \
         Unset SCRYER_EXPORT_SCRIPT to export with the viewer built into this binary instead."
    ))
}

/// Run the real script against a throwaway project holding just the layer asked
/// for, so it always exports "the committed model" of something consistent.
async fn run_script(script: &Path, model: &str) -> CommandResult<String> {
    let repo_root = script
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| CommandError::failed("the export script is not inside a checkout"))?;

    let work = tempfile::tempdir()
        .map_err(|e| CommandError::failed(format!("no temporary directory: {e}")))?;
    let staged = work.path().join("project");
    std::fs::create_dir_all(staged.join(".scryer"))
        .map_err(|e| CommandError::failed(format!("could not stage the model: {e}")))?;
    std::fs::write(staged.join(".scryer").join("model.scry"), model)
        .map_err(|e| CommandError::failed(format!("could not stage the model: {e}")))?;
    let out = work.path().join("scryer-diagram.html");

    let run = tokio::process::Command::new("node")
        .arg(script)
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
                "the export script did not finish within {}s",
                EXPORT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| {
            CommandError::failed(format!(
                "could not run the export script at {} (is node installed?): {e}",
                script.display()
            ))
        })?;
    if !output.status.success() {
        // The script's own diagnosis, not just its exit code.
        return Err(CommandError::failed(format!(
            "the export script at {} failed: {}",
            script.display(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A model with every character that could break out of the literal it is
    /// put into — a backtick, a template substitution, backslashes, quotes,
    /// newlines and non-ASCII. A node really can be named any of these.
    const HOSTILE: &str = concat!(
        r#"{"version":"0.3","nodes":[{"id":"node-1","kind":"system","name":"tick ` "#,
        r#"dollar ${x} back\\slash \"quoted\" ünïcode "#,
        "\n  newline </script> \u{2028}\"}],\"links\":[],\"groups\":[]}"
    );

    /// The shipped artifact carries exactly one model slot, in exactly one
    /// quoting. Cheap, always-on, and the thing that breaks first if the
    /// artifact is regenerated by a bundler that emits a different shape.
    #[test]
    fn the_shipped_viewer_carries_exactly_one_model_slot() {
        let quoted = TEMPLATE.matches(&format!("\"{SENTINEL}\"")).count();
        let templated = TEMPLATE.matches(&format!("`{SENTINEL}`")).count();
        assert_eq!(
            (quoted, templated),
            (1, 0),
            "the committed artifact's slot changed shape"
        );
        assert!(
            TEMPLATE.len() > 100_000,
            "the whole viewer is in there, not a stub"
        );
        assert!(TEMPLATE.trim_start().starts_with("<!doctype html"));
    }

    /// `resp-bs0y4b` (unit half) — a hostile model goes in and comes back out
    /// of the literal byte-identical, so the export carries what the project
    /// actually says rather than whatever survived the escaping.
    #[test]
    fn a_hostile_model_survives_both_quotings_intact() {
        // Quoted: the emitted literal is JSON, so it decodes back exactly.
        let out = render(&format!("x = \"{SENTINEL}\";"), HOSTILE).unwrap();
        let literal = out.trim_start_matches("x = ").trim_end_matches(';');
        assert_eq!(
            serde_json::from_str::<String>(literal).expect("a well-formed JS string literal"),
            HOSTILE,
        );

        // Templated: nothing is left that would close the literal or open a
        // substitution, and un-escaping returns the original.
        let out = render(&format!("x = `{SENTINEL}`;"), HOSTILE).unwrap();
        let literal = out
            .trim_start_matches("x = `")
            .trim_end_matches("`;")
            .to_string();
        assert_eq!(
            (
                first_unescaped(&literal, "`"),
                first_unescaped(&literal, "${")
            ),
            (None, None),
            "nothing is left that could close the literal or open a substitution"
        );
        let back = literal
            .replace("\\${", "${")
            .replace("\\`", "`")
            .replace("\\\\", "\\");
        assert_eq!(back, HOSTILE);
    }

    /// The byte offset of the first `needle` that is NOT backslash-escaped, if
    /// there is one. An odd run of backslashes before it means it is escaped.
    fn first_unescaped(text: &str, needle: &str) -> Option<usize> {
        text.match_indices(needle)
            .find(|(i, _)| text[..*i].chars().rev().take_while(|b| *b == '\\').count() % 2 == 0)
            .map(|(i, _)| i)
    }

    /// A template whose slot is missing, doubled, or ambiguous refuses loudly —
    /// it never ships a file that looks fine and is broken.
    #[test]
    fn a_template_without_exactly_one_slot_refuses() {
        for broken in [
            "no slot at all".to_string(),
            format!("\"{SENTINEL}\" and \"{SENTINEL}\""),
            format!("\"{SENTINEL}\" and `{SENTINEL}`"),
            // Bare, unquoted: not a literal we can safely substitute into.
            SENTINEL.to_string(),
        ] {
            let err = render(&broken, "{}").unwrap_err();
            assert!(
                err.to_string().contains("does not carry its model slot"),
                "{err}"
            );
        }
    }

    /// `resp-exprfs` — an export that cannot be produced is refused naming what
    /// was found and what is missing, never a stack trace.
    ///
    /// The shape that sent us here: a service built from a cargo git checkout
    /// has the script (the whole repo is checked out) and none of its
    /// dependencies (`node_modules` is not in the repo), so node died on a
    /// module-resolution trace out of a path the caller never chose.
    #[test]
    fn resp_exprfs_an_export_that_cannot_be_produced_names_what_is_missing() {
        use std::ffi::OsString;

        // Nobody asked for the script: the shipped viewer answers, no refusal.
        assert_eq!(script_from(None).unwrap(), None);

        let root = tempfile::tempdir().unwrap();
        let scripts = root.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let script = scripts.join("export-html.mjs");
        let named = |p: &Path| Some(OsString::from(p.as_os_str()));

        // Pointed at nothing.
        let err = script_from(named(&script)).unwrap_err().to_string();
        assert!(err.contains(&script.display().to_string()), "{err}");
        assert!(err.contains("there is no file there"), "{err}");
        assert!(err.contains("SCRYER_EXPORT_SCRIPT"), "{err}");

        // The script, but no viewer beside it.
        std::fs::write(&script, "// the script").unwrap();
        let err = script_from(named(&script)).unwrap_err().to_string();
        assert!(
            err.contains("export-viewer/vite.config.ts is missing"),
            "{err}"
        );

        // The viewer, but no installed dependencies — the cargo checkout.
        std::fs::create_dir_all(root.path().join("export-viewer")).unwrap();
        std::fs::write(
            root.path().join("export-viewer").join("vite.config.ts"),
            "// the config",
        )
        .unwrap();
        let err = script_from(named(&script)).unwrap_err().to_string();
        assert!(err.contains("node_modules/vite is missing"), "{err}");
        assert!(err.contains("pnpm install"), "{err}");
        // And it says how to stop needing any of it.
        assert!(
            err.contains("viewer built into this binary"),
            "the refusal names the way out: {err}"
        );

        // Everything present: accepted.
        std::fs::create_dir_all(root.path().join("node_modules").join("vite")).unwrap();
        assert_eq!(script_from(named(&script)).unwrap(), Some(script));
    }

    /// A layer the project does not have refuses by name, before spending
    /// anything finding out.
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
}
