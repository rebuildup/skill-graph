//! Integration tests for `SKILL_GRAPH_ROOTS` semantics (issue #177):
//! a set-but-invalid operator restriction must degrade to an empty or
//! diagnosed scan — never silently widen to explicit args or defaults.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/skills")
}

fn write_skill(dir: &Path, name: &str) {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: decoy skill for roots tests.\n---\n\n# {name}\n"),
    )
    .unwrap();
}

fn run_discover(args: &[&str], envs: &[(&str, &str)], cwd: &Path) -> (Value, String, i32) {
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("HOME", cwd) // isolate HOME-rooted defaults
        .envs(envs.iter().copied())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().expect("spawn skill-graph");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let json = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    (json, stderr, out.status.code().unwrap_or(-1))
}

fn skill_names(json: &Value) -> Vec<String> {
    json["skills"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// env set to a nonexistent dir + an explicit --root arg: the explicit
/// arg must NOT widen the operator restriction — result is empty with
/// a diagnostic error.
#[test]
fn invalid_env_does_not_widen_to_explicit_args() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = fixture_root();
    let env_val = tmp.path().join("does-not-exist");
    let (json, _err, rc) = run_discover(
        &[
            "discover",
            "--root",
            fixture.to_str().unwrap(),
            "--format",
            "json",
        ],
        &[("SKILL_GRAPH_ROOTS", env_val.to_str().unwrap())],
        tmp.path(),
    );
    assert_eq!(rc, 0);
    assert!(
        skill_names(&json).is_empty(),
        "explicit --root leaked past invalid env restriction: {json}"
    );
    let errors = json["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("root not found")),
        "expected diagnostic for invalid env root: {json}"
    );
}

/// env pointing at an existing-but-empty dir must not fall back to
/// cwd/HOME defaults that contain skills.
#[test]
fn empty_valid_env_does_not_fall_back_to_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    // Decoy skill inside cwd/skills — would be found by default roots.
    write_skill(&tmp.path().join("skills"), "decoy-skill");
    let empty_root = tmp.path().join("empty-roots");
    std::fs::create_dir_all(&empty_root).unwrap();

    let (json, _err, rc) = run_discover(
        &["discover", "--format", "json"],
        &[("SKILL_GRAPH_ROOTS", empty_root.to_str().unwrap())],
        tmp.path(),
    );
    assert_eq!(rc, 0);
    assert!(
        skill_names(&json).is_empty(),
        "default roots leaked past env restriction: {json}"
    );
}

/// unset env → normal default discovery still works (cwd/skills).
#[test]
fn unset_env_uses_default_roots() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(&tmp.path().join("skills"), "decoy-skill");

    let (json, _err, rc) = run_discover(&["discover", "--format", "json"], &[], tmp.path());
    assert_eq!(rc, 0);
    assert!(
        skill_names(&json).iter().any(|n| n == "decoy-skill"),
        "default discovery broken: {json}"
    );
}

/// empty-string env is treated as unset → defaults apply.
#[test]
fn empty_env_string_is_unset() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(&tmp.path().join("skills"), "decoy-skill");

    let (json, _err, rc) = run_discover(
        &["discover", "--format", "json"],
        &[("SKILL_GRAPH_ROOTS", "")],
        tmp.path(),
    );
    assert_eq!(rc, 0);
    assert!(
        skill_names(&json).iter().any(|n| n == "decoy-skill"),
        "empty env should behave as unset: {json}"
    );
}

/// partially valid env: valid entries scanned, invalid ones diagnosed,
/// and out-of-scope defaults still excluded.
#[test]
fn partial_valid_env_scopes_to_valid_entries_only() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(&tmp.path().join("skills"), "decoy-skill");
    let good = tmp.path().join("good-root");
    write_skill(&good, "env-skill");
    let bad = tmp.path().join("missing-root");

    let env_val = format!("{}:{}", bad.display(), good.display());
    let (json, _err, rc) = run_discover(
        &["discover", "--format", "json"],
        &[("SKILL_GRAPH_ROOTS", env_val.as_str())],
        tmp.path(),
    );
    assert_eq!(rc, 0);
    let names = skill_names(&json);
    assert!(names.iter().any(|n| n == "env-skill"), "{json}");
    assert!(
        !names.iter().any(|n| n == "decoy-skill"),
        "cwd default leaked: {json}"
    );
    let errors = json["errors"].as_array().cloned().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("root not found")),
        "invalid entry not diagnosed: {json}"
    );
}

/// env set to only separators resolves to an empty restricted scope —
/// no widening, empty result.
#[test]
fn separators_only_env_is_empty_restriction() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(&tmp.path().join("skills"), "decoy-skill");

    let (json, _err, rc) = run_discover(
        &["discover", "--format", "json"],
        &[("SKILL_GRAPH_ROOTS", "::")],
        tmp.path(),
    );
    assert_eq!(rc, 0);
    assert!(
        skill_names(&json).is_empty(),
        "separators-only env widened scope: {json}"
    );
}
