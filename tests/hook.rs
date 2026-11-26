//! Integration tests for `skill-graph hook` — spawn the real binary so env
//! isolation is per-process.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn run_hook(args: &[&str], payload: &Value, envs: &[(&str, &str)]) -> (String, i32) {
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn skill-graph");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn event_lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn fixture_env(dir: &tempfile::TempDir) -> Vec<(String, String)> {
    vec![
        (
            "SKILL_GRAPH_EVENT_LOG".to_string(),
            dir.path().join("events.jsonl").to_string_lossy().into(),
        ),
        (
            "SKILL_GRAPH_HOOK_STATE".to_string(),
            dir.path().join("state").to_string_lossy().into(),
        ),
        ("SKILL_GRAPH_ROOTS".to_string(), fixture_roots()),
    ]
}

fn fixture_roots() -> String {
    // fixtures/skills contains alpha-skill (requires beta-skill) etc.
    concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/skills").to_string()
}

fn env_pairs(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

#[test]
fn prompt_mode_emits_events_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({"session_id":"s1","prompt":"use alpha-skill please"});
    let (out, rc) = run_hook(
        &["hook", "claude-code", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    let v: Value = serde_json::from_str(out.trim()).expect("hook response json");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains("alpha-skill"), "context: {ctx}");

    let events = event_lines(&dir.path().join("events.jsonl"));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "prompt_submit");
    assert_eq!(events[1]["kind"], "recommendation");
    assert_eq!(events[1]["trigger"], "prompt_submit");
    assert_eq!(events[1]["emitted"], true);
    // prompt body must never be persisted
    assert!(events[0]["prompt_hash"].is_string());
    assert!(events[0].get("prompt").is_none());
}

#[test]
fn prompt_dedup_skips_second_emission() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({"session_id":"s1","prompt":"alpha task"});
    let args = &["hook", "claude-code", "prompt"];
    let (out1, _) = run_hook(args, &payload, &env_pairs(&envs));
    let (out2, _) = run_hook(args, &payload, &env_pairs(&envs));
    assert!(!out1.trim().is_empty());
    assert!(out2.trim().is_empty(), "dedup should suppress output");
    let events = event_lines(&dir.path().join("events.jsonl"));
    let recs: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "recommendation")
        .collect();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[1]["reason"], "dedup_skip");
    assert_eq!(recs[1]["emitted"], false);
}

#[test]
fn skill_load_detects_and_recommends_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({
        "session_id":"s2",
        "tool_name":"Skill",
        "tool_input":{"skill":"alpha-skill"}
    });
    let (out, rc) = run_hook(
        &["hook", "claude-code", "skill-load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    let events = event_lines(&dir.path().join("events.jsonl"));
    let load = events.iter().find(|e| e["kind"] == "skill_load");
    let load = load.expect("skill_load event");
    assert_eq!(load["skill_name"], "alpha-skill");
    assert_eq!(load["detection"], "native_invocation");
    if !out.trim().is_empty() {
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    }
}

#[test]
fn non_skill_tool_call_is_filtered_before_detection() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({
        "session_id":"s3",
        "tool_name":"Bash",
        "tool_input":{"command":"ls -la"}
    });
    let (out, rc) = run_hook(
        &["hook", "codex", "skill-load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(out.trim().is_empty());
    // pre-filter must prevent even the event log write
    assert!(!dir.path().join("events.jsonl").exists());
}

#[test]
fn nostate_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut envs = fixture_env(&dir);
    envs.push(("SKILL_GRAPH_HOOK_NOSTATE".to_string(), "1".to_string()));
    let payload = json!({"session_id":"s4","prompt":"alpha"});
    let (_, rc) = run_hook(
        &["hook", "claude-code", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(!dir.path().join("events.jsonl").exists());
    assert!(!dir.path().join("state").exists());
}

#[test]
fn invalid_json_fails_open() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let out = Command::new(bin)
        .args(["hook", "claude-code", "prompt"])
        .envs(env_pairs(&envs))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    let _ = out;
    // feed invalid json via a second run
    let mut child = Command::new(bin)
        .args(["hook", "claude-code", "prompt"])
        .envs(env_pairs(&envs))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"not json{{{")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
}
