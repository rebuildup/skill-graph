//! Integration tests for CLI argument surfaces — spawn the real binary.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/skills")
}

fn run_recommend(args: &[&str], stdin: Option<&str>) -> (String, String, i32) {
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn skill-graph");
    if let Some(input) = stdin {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn recommend_query_stdin_reads_query_from_stdin() {
    let root = fixture_root();
    let (out, err, rc) = run_recommend(
        &[
            "recommend",
            "--query-stdin",
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
        ],
        Some("alpha skill"),
    );
    assert_eq!(rc, 0, "{err}");
    let recs: Value = serde_json::from_str(out.trim()).expect("json output");
    assert!(
        recs.as_array()
            .unwrap()
            .iter()
            .any(|r| { r["skill"]["name"].as_str() == Some("alpha-skill") }),
        "{out}"
    );
}

#[test]
fn recommend_query_stdin_handles_leading_dash_prompt() {
    // A prompt starting with `-` would be parsed as a flag if passed as a
    // positional arg; stdin delivery must carry it verbatim.
    let root = fixture_root();
    let (out, err, rc) = run_recommend(
        &[
            "recommend",
            "--query-stdin",
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
        ],
        Some("-fix the alpha skill bug"),
    );
    assert_eq!(rc, 0, "{err}");
    let recs: Value = serde_json::from_str(out.trim()).expect("json output");
    assert!(
        recs.as_array()
            .unwrap()
            .iter()
            .any(|r| { r["skill"]["name"].as_str() == Some("alpha-skill") }),
        "{out}"
    );
}

#[test]
fn recommend_rejects_missing_or_double_query() {
    let root = fixture_root();
    let root = root.to_str().unwrap();
    let (_, err, rc) = run_recommend(&["recommend", "--root", root], None);
    assert_ne!(rc, 0);
    assert!(err.contains("query"), "{err}");
    let (_, err, rc) = run_recommend(
        &["recommend", "q", "--query-stdin", "--root", root],
        Some("q"),
    );
    assert_ne!(rc, 0);
    assert!(err.contains("not both") || err.contains("either"), "{err}");
}
