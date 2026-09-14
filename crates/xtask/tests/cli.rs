//! The dev-tasks CLI's dispatch: each invocation routes to its task, and an
//! unknown (or missing) task prints usage and fails.

use std::process::Command;

fn xtask() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
}

/// No task, or an unknown one, is dispatched nowhere: usage on stderr, exit 1.
#[test]
fn an_unknown_task_prints_usage_and_fails() {
    for args in [&[][..], &["frobnicate"][..]] {
        let out = xtask().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Usage:"));
    }
}

/// `validate-model` dispatches to the validation task: it reads the project's
/// model and runs structural and source-coverage validation against it —
/// a clean model reports clean and exits 0, a missing one fails.
#[test]
fn validate_model_reads_and_validates_the_projects_model() {
    let dir = tempfile::tempdir().unwrap();
    let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
    scryer_core::write_model_at(&r, &scryer_core::ScryModel::new()).unwrap();

    let out = xtask()
        .args(["validate-model", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("clean"));

    let empty = tempfile::tempdir().unwrap();
    let out = xtask()
        .args(["validate-model", empty.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "no model to read must fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Failed to read"));
}

/// A workspace whose formatting the gate can be pointed at: a `main` holding
/// one well-formatted and two badly formatted files, one of them behind the
/// pin's fence. Returns the directory; the caller commits its own change on a
/// branch off `main`, which is the change the gate is scoped to.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    write(
        p,
        "Cargo.toml",
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    );
    write(
        p,
        "rustfmt.toml",
        "edition = \"2021\"\nstyle_edition = \"2021\"\nmax_width = 100\n\
         # a generated crate's whitespace is not ours to choose\n\
         ignore = [\"generated\"]\n",
    );
    // Never part of any change below, and never formatted: naming it is the
    // gate reaching past the change.
    write(p, "untouched.rs", UNFORMATTED);
    write(p, "touched.rs", FORMATTED);
    write(p, "other.rs", FORMATTED);
    write(p, "generated/parser.rs", UNFORMATTED);
    git(p, &["init", "-b", "main", "-q"]);
    git(p, &["add", "-A"]);
    commit(p, "base");
    git(p, &["checkout", "-q", "-b", "work"]);
    dir
}

const UNFORMATTED: &str = "pub fn  n( )->i32{1+1}\n";
const FORMATTED: &str = "pub fn n() -> i32 {\n    1 + 1\n}\n";

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let at = root.join(rel);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::fs::write(at, body).unwrap();
}

fn git(root: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {:?}", out);
}

fn commit(root: &std::path::Path, message: &str) {
    git(
        root,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

fn fmt_check(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("fmt-check")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

/// resp-b5djjs — the gate is scoped to the change: the Rust files that differ
/// from the base ref, and the working tree's own only when it is asked. A file
/// the change never touched stays unformatted without failing anything.
#[test]
fn resp_b5djjs_checks_the_changed_files_and_the_working_tree_when_asked() {
    let dir = fixture();
    let p = dir.path();

    // A committed change that touches one file, formatted. `untouched.rs` is
    // unformatted throughout and is not this change's business.
    std::fs::write(
        p.join("touched.rs"),
        format!("{FORMATTED}\npub const N: i32 = 1;\n"),
    )
    .unwrap();
    git(p, &["add", "-A"]);
    commit(p, "touch one file");
    let out = fmt_check(p, &[]);
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{all}");
    assert!(
        !all.contains("untouched.rs"),
        "the gate reached past the change: {all}"
    );

    // A file that exists only in the working tree is not part of the committed
    // change, so it is out of scope until the gate is asked to look there.
    std::fs::write(p.join("fresh.rs"), UNFORMATTED).unwrap();
    let out = fmt_check(p, &[]);
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{all}");
    assert!(!all.contains("fresh.rs"), "{all}");
    let out = fmt_check(p, &["--working"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("fresh.rs"));
}

/// resp-z9a0ex — a file this change touches that does not match the pin fails
/// the run, and every such file is named, not just the first.
#[test]
fn resp_z9a0ex_refuses_the_run_naming_every_file_that_falls_short() {
    let dir = fixture();
    let p = dir.path();
    std::fs::write(p.join("touched.rs"), UNFORMATTED).unwrap();
    std::fs::write(p.join("other.rs"), UNFORMATTED).unwrap();
    git(p, &["add", "-A"]);
    commit(p, "two unformatted files");

    let out = fmt_check(p, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("touched.rs"), "{err}");
    assert!(err.contains("other.rs"), "{err}");
    assert!(!err.contains("untouched.rs"), "{err}");
}

/// resp-tm5qwy — a path the pin's `ignore` list fences off is skipped even
/// when the change touches it. Stable rustfmt drops `ignore` on the floor, so
/// the gate honours the list itself or the fence is decorative.
#[test]
fn resp_tm5qwy_skips_the_paths_the_pin_fences_off() {
    let dir = fixture();
    let p = dir.path();
    std::fs::write(
        p.join("generated/parser.rs"),
        format!("{UNFORMATTED}pub const M: i32 = 2;\n"),
    )
    .unwrap();
    git(p, &["add", "-A"]);
    commit(p, "regenerate the parser");

    let out = fmt_check(p, &[]);
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{all}");
    assert!(all.contains("1 fenced off"), "{all}");
}
