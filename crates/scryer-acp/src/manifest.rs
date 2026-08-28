//! The per-run record written beside a session's transcript.
//!
//! A session runs with `--no-session-persistence` and takes its prompt over
//! stdin, so the transcript the runtime tees holds everything the agent SAID
//! and nothing about what it was ASKED or what the run was for. Anything
//! outside the app reading those transcripts — the Hapi hub surfaces them as
//! sessions — has no way to name a run or show its instruction.
//!
//! The manifest closes that gap: one JSON file per session, `session-{id}.json`
//! next to `session-{id}.jsonl`, written when the session starts and rewritten
//! once when it ends. Written best-effort — a failure here never touches the
//! session itself.

use crate::events::Usage;
use std::path::{Path, PathBuf};

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// The record of one agent run: what it was asked, and how it went.
///
/// `schema` is the compatibility signal for outside readers — bump it when a
/// field changes meaning, not when one is added.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunManifest {
    pub schema: u32,
    /// The session id the app knows this run by.
    pub session_id: String,
    /// The runtime's per-process counter — also the transcript's filename stem.
    pub run: u64,
    /// Transcript filename, relative to this file. Absent in ACP mode, which
    /// carries no transcript of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// What this run was started to do, in the orchestrator's words.
    pub label: String,
    pub project: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effort: String,
    pub started_at: u64,
    /// The prompt the agent was given, in full.
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// Why the run failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Where a run's manifest lives: beside its transcript, same stem.
pub fn manifest_path(cwd: &str, id: u64) -> PathBuf {
    Path::new(cwd)
        .join(".scryer")
        .join("build-logs")
        .join(format!("session-{id}.json"))
}

/// Write the opening record. Returns the path so the caller can finish it
/// without recomputing, and `None` when the record could not be written —
/// there is nothing to finish in that case.
#[allow(clippy::too_many_arguments)]
pub fn start_run_manifest(
    cwd: &str,
    id: u64,
    session_id: &str,
    label: &str,
    agent: &str,
    model: &str,
    effort: &str,
    prompt: &str,
    has_transcript: bool,
) -> Option<PathBuf> {
    let manifest = RunManifest {
        schema: 1,
        session_id: session_id.to_string(),
        run: id,
        transcript: has_transcript.then(|| format!("session-{id}.jsonl")),
        label: label.to_string(),
        project: cwd.to_string(),
        agent: agent.to_string(),
        model: model.to_string(),
        effort: effort.to_string(),
        started_at: now_secs(),
        prompt: prompt.to_string(),
        ended_at: None,
        outcome: None,
        error: None,
        usage: None,
    };
    let path = manifest_path(cwd, id);
    write_manifest(&path, &manifest).then_some(path)
}

/// Close the record out: how the run ended and what it consumed. Re-reads the
/// opening record rather than holding it, so a run that was never recorded
/// (unwritable directory) stays silently absent instead of appearing here with
/// half its fields.
pub fn finish_run_manifest(
    path: &Path,
    outcome: RunOutcome,
    error: Option<String>,
    usage: Option<Usage>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut manifest) = serde_json::from_str::<RunManifest>(&text) else {
        return;
    };
    manifest.ended_at = Some(now_secs());
    manifest.outcome = Some(outcome);
    manifest.error = error;
    manifest.usage = usage;
    write_manifest(path, &manifest);
}

/// Serialize through a temp file and rename, so a reader tailing the directory
/// never sees a half-written record.
fn write_manifest(path: &Path, manifest: &RunManifest) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let Ok(json) = serde_json::to_string_pretty(manifest) else {
        return false;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(dir: &tempfile::TempDir, id: u64) -> serde_json::Value {
        let text =
            std::fs::read_to_string(manifest_path(&dir.path().to_string_lossy(), id)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// A run's record names the task and carries the whole prompt, so a reader
    /// that only has the transcript can still say what the run was for.
    #[test]
    fn a_started_run_records_its_task_and_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();

        let path = start_run_manifest(
            &cwd, 3, "sync-42", "Drift check: Domain Designer", "claudeCode", "opus", "high",
            "You have the scryer MCP server", true,
        );

        assert!(path.is_some(), "a writable project records the run");
        let v = manifest_json(&dir, 3);
        assert_eq!(v["label"], "Drift check: Domain Designer");
        assert_eq!(v["prompt"], "You have the scryer MCP server");
        assert_eq!(v["sessionId"], "sync-42");
        assert_eq!(v["run"], 3);
        assert_eq!(v["transcript"], "session-3.jsonl");
        assert_eq!(v["schema"], 1);
        assert!(v["startedAt"].as_u64().unwrap() > 0);
        assert!(v.get("outcome").is_none(), "an open run has no outcome yet");
    }

    /// The record sits next to the transcript it describes — that pairing is
    /// how a reader matches one to the other.
    #[test]
    fn the_record_sits_beside_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = start_run_manifest(&cwd, 7, "sync-1", "Fill container", "codex", "", "", "p", true).unwrap();

        assert_eq!(path.file_name().unwrap(), "session-7.json");
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "build-logs");
        assert!(path.parent().unwrap().join("session-7.jsonl").parent().unwrap().exists());
    }

    /// An ACP run has no transcript of its own, and says so rather than
    /// pointing at a file that will never exist.
    #[test]
    fn an_acp_run_claims_no_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        start_run_manifest(&cwd, 1, "sync-1", "Preview fixture", "copilot", "", "medium", "p", false);

        let v = manifest_json(&dir, 1);
        assert!(v.get("transcript").is_none());
    }

    /// Finishing stamps how the run ended and what it cost, leaving everything
    /// the opening record said intact.
    #[test]
    fn finishing_records_the_outcome_and_cost() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = start_run_manifest(&cwd, 2, "sync-9", "Drift check", "claudeCode", "opus", "high", "the prompt", true).unwrap();

        let usage = Usage { input_tokens: 10, output_tokens: 20, cache_creation_input_tokens: 30, cache_read_input_tokens: 40, cost_usd: 1.5 };
        finish_run_manifest(&path, RunOutcome::Completed, None, Some(usage));

        let v = manifest_json(&dir, 2);
        assert_eq!(v["outcome"], "completed");
        assert_eq!(v["usage"]["outputTokens"], 20);
        assert_eq!(v["usage"]["costUsd"], 1.5);
        assert_eq!(v["prompt"], "the prompt", "the opening record survives the fold");
        assert!(v["endedAt"].as_u64().unwrap() >= v["startedAt"].as_u64().unwrap());
    }

    /// A failure records why, so a reader can show the ending rather than an
    /// unexplained stop.
    #[test]
    fn a_failed_run_records_why() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = start_run_manifest(&cwd, 0, "s", "l", "claudeCode", "", "", "p", true).unwrap();

        finish_run_manifest(&path, RunOutcome::Failed, Some("exit code 1".into()), None);

        let v = manifest_json(&dir, 0);
        assert_eq!(v["outcome"], "failed");
        assert_eq!(v["error"], "exit code 1");
    }

    /// Finishing a run that was never recorded writes nothing — a half record
    /// is worse than none, since a reader would open a session for it.
    #[test]
    fn finishing_an_unrecorded_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("session-5.json");

        finish_run_manifest(&missing, RunOutcome::Completed, None, None);

        assert!(!missing.exists());
    }
}
