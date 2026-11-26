//! Integration tests for `skill-graph detect` — spawn the real binary.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn run_detect(args: &[&str], stdin: &str) -> (String, i32) {
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn skill-graph");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn skill_read_payload() -> Value {
    json!({
        "tool_name": "Read",
        "tool_input": {"file_path": "/repo/skills/alpha-skill/SKILL.md"},
        "tool_response": {"file": {"content": "---\nname: alpha-skill\ndescription: x\n---\nbody"}},
        "session_id": "sess-1"
    })
}

#[test]
fn detect_emits_skill_load_event_for_skill_md_read() {
    let (out, rc) = run_detect(
        &["detect", "--runtime", "claude-code", "--via", "hook"],
        &skill_read_payload().to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let event: Value = serde_json::from_str(out.trim()).expect("event json");
    assert_eq!(event["kind"], "skill_load");
    assert_eq!(event["runtime"], "claude-code");
    assert_eq!(event["observed_via"], "hook");
    assert_eq!(event["session_id"], "sess-1");
    assert_eq!(event["skill_name"], "alpha-skill");
}

#[test]
fn detect_accepts_opencode_plugin_shape() {
    let payload = json!({
        "tool": "read",
        "input": {"path": "/repo/skills/beta-skill/SKILL.md"},
        "result": "file contents here",
        "sessionID": "oc-9"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "opencode", "--via", "plugin"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let event: Value = serde_json::from_str(out.trim()).expect("event json");
    assert_eq!(event["kind"], "skill_load");
    assert_eq!(event["runtime"], "opencode");
    assert_eq!(event["observed_via"], "plugin");
    assert_eq!(event["session_id"], "oc-9");
}

#[test]
fn detect_prints_nothing_without_skill_signal() {
    let payload = json!({
        "tool_name": "Write",
        "tool_input": {"file_path": "/repo/src/main.rs", "content": "fn main() {}"},
        "tool_response": "ok",
        "session_id": "s"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "codex", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0);
    assert!(out.trim().is_empty(), "unexpected output: {out}");
}

#[test]
fn detect_rejects_unknown_runtime_and_via() {
    let payload = skill_read_payload().to_string();
    let (_, rc) = run_detect(&["detect", "--runtime", "vim", "--via", "hook"], &payload);
    assert_ne!(rc, 0);
    let (_, rc) = run_detect(
        &["detect", "--runtime", "codex", "--via", "psychic"],
        &payload,
    );
    assert_ne!(rc, 0);
}

#[test]
fn detect_rejects_invalid_json() {
    let (_, rc) = run_detect(
        &["detect", "--runtime", "codex", "--via", "hook"],
        "{not json",
    );
    assert_ne!(rc, 0);
}

#[test]
fn detect_via_transcript_and_db_accepted() {
    for via in ["transcript", "db"] {
        let (out, rc) = run_detect(
            &["detect", "--runtime", "opencode", "--via", via],
            &skill_read_payload().to_string(),
        );
        assert_eq!(rc, 0, "via={via}: {out}");
        let event: Value = serde_json::from_str(out.trim()).expect("event json");
        assert_eq!(event["observed_via"], via);
    }
}
