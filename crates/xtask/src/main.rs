use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    let task = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let debug = args.iter().any(|a| a == "--debug");

    match task {
        "build-sidecar" => build_sidecar(!debug),
        "export-template" => export_template(args.iter().any(|a| a == "--check")),
        "validate-model" => validate_model(args.get(2).map(PathBuf::from)),
        "fmt-check" => fmt_check(&args[2..]),
        _ => {
            eprintln!(
                "Usage:\n  cargo run -p xtask -- build-sidecar [--debug]\n  \
                 cargo run -p xtask -- validate-model [project-path]\n  \
                 cargo run -p xtask -- export-template [--check]\n  \
                 cargo run -p xtask -- fmt-check [--base <ref>] [--working]"
            );
            std::process::exit(1);
        }
    }
}

/// The token the export viewer is baked with in place of a project's model.
/// `scryer-app`'s `export_html` finds it and substitutes the real thing, so the
/// engine exports with no node and no checkout. Must match `export::SENTINEL`.
const SENTINEL: &str = "@@SCRYER_MODEL_SENTINEL@@";

/// Where the committed artifact lives, relative to the workspace root.
const TEMPLATE_PATH: &str = "crates/scryer-app/assets/export-viewer.html";

/// Rebuild the export viewer against the sentinel and commit it — or, with
/// `--check`, verify that the committed artifact is what a rebuild produces.
///
/// The artifact is generated, ~600 KB, and checked in, so it can go stale the
/// moment the UI changes and nobody would notice: the export would keep working
/// and keep shipping last month's viewer. `--check` is what notices. Needs node
/// and an installed checkout — the dependency the artifact exists to spare
/// everyone else.
fn export_template(check: bool) {
    let root = workspace_root();
    let script = root.join("scripts").join("export-html.mjs");
    if !script.is_file() {
        eprintln!("no export script at {}", script.display());
        std::process::exit(1);
    }

    let work = tempfile::tempdir().expect("no temporary directory");
    let project = work.path().join("sentinel");
    std::fs::create_dir_all(project.join(".scryer")).expect("could not stage the sentinel project");
    // The whole model file IS the token, so substituting it replaces the model
    // entire rather than patching something around it.
    std::fs::write(project.join(".scryer").join("model.scry"), SENTINEL)
        .expect("could not write the sentinel model");
    let built = work.path().join("export-viewer.html");

    println!("Building the export viewer against the sentinel…");
    let status = Command::new("node")
        .arg(&script)
        .arg(&project)
        .arg("-o")
        .arg(&built)
        .current_dir(&root)
        .status()
        .expect("failed to run node (is it installed, with `pnpm install` run in this checkout?)");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    let fresh = std::fs::read(&built).expect("the build produced no readable file");
    let occurrences = String::from_utf8_lossy(&fresh).matches(SENTINEL).count();
    if occurrences != 1 {
        eprintln!(
            "the built viewer holds {occurrences} occurrences of {SENTINEL}, expected exactly 1 —              the bundler's output shape changed and the substitution would be wrong"
        );
        std::process::exit(1);
    }

    let dst = root.join(TEMPLATE_PATH);
    if check {
        let committed = std::fs::read(&dst).unwrap_or_default();
        if committed == fresh {
            println!("{TEMPLATE_PATH} is current ({} bytes).", fresh.len());
            return;
        }
        eprintln!(
            "{TEMPLATE_PATH} is STALE: a rebuild produces {} bytes, the committed artifact has {}.\n\
             Run `cargo run -p xtask -- export-template` and commit the result.",
            fresh.len(),
            committed.len()
        );
        std::process::exit(2);
    }

    std::fs::create_dir_all(dst.parent().expect("assets dir has a parent"))
        .expect("could not create the assets directory");
    std::fs::write(&dst, &fresh).expect("could not write the template");
    println!("Wrote {TEMPLATE_PATH} ({} bytes).", fresh.len());
}

/// The default base a change is measured against when none is named.
const DEFAULT_BASE: &str = "main";

/// The formatting gate, scoped to the change rather than to the tree.
///
/// The pin in `rustfmt.toml` says what formatted means; this says WHICH files
/// have to be it. A change's own files do — `git diff --name-only
/// <base>...HEAD`, plus the working tree's own changes with `--working`. A
/// file the change would not otherwise touch is left alone, formatting and
/// blame both, because a tree-wide sweep buries the change it travels with and
/// rewrites the history of code nobody edited.
///
/// Exit 0 clean, 1 when a file the change touches is not formatted (each one
/// named), 2 when the invocation itself is wrong — a bad flag or a `--base`
/// that does not resolve, which must be loud rather than silently checking
/// nothing.
fn fmt_check(args: &[String]) {
    let mut base: Option<String> = None;
    let mut base_was_named = false;
    let mut working = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--working" => working = true,
            "--base" => match it.next() {
                Some(r) => {
                    base = Some(r.clone());
                    base_was_named = true;
                }
                None => usage_error("--base needs a git ref"),
            },
            other => match other.strip_prefix("--base=") {
                Some(r) => {
                    base = Some(r.to_string());
                    base_was_named = true;
                }
                None => usage_error(&format!("unknown argument '{other}'")),
            },
        }
    }
    let root = workspace_root();
    let base = base.unwrap_or_else(|| DEFAULT_BASE.to_string());

    let mut touched: BTreeSet<PathBuf> = BTreeSet::new();
    if git_ok(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base}^{{commit}}"),
        ],
    ) {
        touched.extend(git_paths(
            &root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=ACMR",
                &format!("{base}...HEAD"),
                "--",
                "*.rs",
            ],
        ));
    } else if base_was_named {
        eprintln!("fmt-check: '{base}' does not resolve to a commit");
        std::process::exit(2);
    } else {
        println!("fmt-check: no '{base}' to compare against; checking the working tree only.");
        working = true;
    }
    if working {
        touched.extend(git_paths(
            &root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=ACMR",
                "HEAD",
                "--",
                "*.rs",
            ],
        ));
        touched.extend(git_paths(
            &root,
            &["ls-files", "--others", "--exclude-standard", "--", "*.rs"],
        ));
    }

    // The pin's own fence, honoured here because rustfmt will not honour it:
    // `ignore` is a nightly-only option, so stable rustfmt reads it, warns, and
    // formats the file anyway. A fence nobody enforces is worse than none.
    let fenced = fenced_paths(&root);
    let mut skipped = 0usize;
    let files: Vec<PathBuf> = touched
        .into_iter()
        .filter(|f| {
            if fenced.iter().any(|p| f.starts_with(p)) {
                skipped += 1;
                return false;
            }
            root.join(f).is_file()
        })
        .collect();

    if files.is_empty() {
        println!("fmt-check: no changed Rust files to check ({skipped} fenced off).");
        return;
    }

    let status = Command::new("rustfmt")
        .args(["--edition", "2021", "--check"])
        .args(&files)
        .current_dir(&root)
        .status()
        .expect("failed to run rustfmt (is the rustfmt component installed?)");
    if status.success() {
        println!(
            "fmt-check: {} changed Rust file(s) match the pin ({skipped} fenced off).",
            files.len()
        );
        return;
    }
    eprintln!(
        "\nfmt-check: these files are part of this change and do not match the pinned formatting:"
    );
    for f in &files {
        eprintln!("  {}", f.display());
    }
    eprintln!("Run `rustfmt --edition 2021` over them (or `cargo fmt -- <file>…`) and commit.");
    std::process::exit(1);
}

fn usage_error(why: &str) -> ! {
    eprintln!("{why}\nusage: cargo run -p xtask -- fmt-check [--base <ref>] [--working]");
    std::process::exit(2);
}

/// The prefixes `rustfmt.toml`'s `ignore` list fences off, project-relative.
/// Absent or unparseable, nothing is fenced — the gate's default is to check.
fn fenced_paths(root: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(root.join("rustfmt.toml")).unwrap_or_default();
    let uncommented: String = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let Some(after) = uncommented.split_once("ignore") else {
        return Vec::new();
    };
    let Some(list) = after.1.split_once('[').and_then(|(_, r)| r.split_once(']')) else {
        return Vec::new();
    };
    list.0
        .split(',')
        .map(|e| e.trim().trim_matches(['"', '\'']).trim())
        .filter(|e| !e.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Whether a git invocation succeeds, output discarded.
fn git_ok(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The project-relative paths a git invocation lists, one per line.
fn git_paths(root: &Path, args: &[&str]) -> Vec<PathBuf> {
    let out = match Command::new("git").args(args).current_dir(root).output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn validate_model(project: Option<PathBuf>) {
    let project = project.unwrap_or_else(workspace_root);
    let model_ref = scryer_core::ModelRef::ProjectLocal(project.clone());
    let model = scryer_core::read_model_at(&model_ref).unwrap_or_else(|e| {
        eprintln!("Failed to read {}: {e}", model_ref.model_path().display());
        std::process::exit(1);
    });
    let mut warnings = scryer_core::validate::validate(&model);
    warnings.extend(scryer_core::validate::validate_coverage(&model, &project));

    println!(
        "{} nodes, {} links, {} groups, {} source mappings, {} boundaries",
        model.nodes.len(),
        model.links.len(),
        model.groups.len(),
        model.source_map.len(),
        model.boundaries.len(),
    );
    if warnings.is_empty() {
        println!("Model is structurally and source-coverage clean.");
        return;
    }
    eprintln!("{} validation warning(s):", warnings.len());
    for warning in warnings {
        eprintln!("- {warning}");
    }
    std::process::exit(2);
}

fn build_sidecar(release: bool) {
    let triple = get_target_triple();
    let root = workspace_root();
    let out_dir = root.join("src-tauri").join("binaries");

    std::fs::create_dir_all(&out_dir).expect("failed to create binaries dir");

    let profile = if release { "release" } else { "debug" };
    println!("Building scryer-mcp ({profile}) for {triple}...");

    let mut args = vec!["build", "-p", "scryer-mcp"];
    if release {
        args.push("--release");
    }

    let status = Command::new("cargo")
        .args(&args)
        .status()
        .expect("failed to run cargo build");

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    let (src_name, dst_name) = if cfg!(windows) {
        (
            "scryer-mcp.exe".to_string(),
            format!("scryer-mcp-{triple}.exe"),
        )
    } else {
        ("scryer-mcp".to_string(), format!("scryer-mcp-{triple}"))
    };

    let src = root.join("target").join(profile).join(&src_name);
    let dst = out_dir.join(&dst_name);

    std::fs::copy(&src, &dst).unwrap_or_else(|e| {
        panic!("failed to copy {} -> {}: {e}", src.display(), dst.display());
    });

    println!("Sidecar copied to {}", dst.display());
}

fn get_target_triple() -> String {
    // Try `rustc --print host-tuple` (stable since 1.84)
    let output = Command::new("rustc")
        .args(["--print", "host-tuple"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let triple = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !triple.is_empty() {
                return triple;
            }
        }
    }

    // Fallback: parse `rustc -vV`
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("failed to run rustc");

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("could not determine host triple from rustc -vV")
        .to_string()
}

fn workspace_root() -> PathBuf {
    // Walk up from current dir to find Cargo.toml with [workspace]
    let mut dir = env::current_dir().expect("no current dir");
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.exists() {
            if let Ok(contents) = std::fs::read_to_string(&manifest) {
                if contents.contains("[workspace]") {
                    return dir;
                }
            }
        }
        if !dir.pop() {
            panic!("could not find workspace root");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolved triple must agree with rustc's own authoritative `host:`
    /// line — the fallback source the function parses when `--print
    /// host-tuple` is unavailable.
    #[test]
    fn target_triple_matches_the_rustc_host() {
        let out = Command::new("rustc")
            .arg("-vV")
            .output()
            .expect("rustc runs");
        let host = String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
            .expect("rustc -vV reports a host");
        assert_eq!(get_target_triple(), host);
    }

    /// Walking up from the test's working directory (crates/xtask) lands on
    /// the directory whose Cargo.toml declares `[workspace]`.
    #[test]
    fn workspace_root_finds_the_workspace_manifest() {
        let root = workspace_root();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("[workspace]"));
        assert!(env::current_dir().unwrap().starts_with(&root));
    }
}
