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
    // malformed payload produces no events and no output
    assert!(out.stdout.is_empty());
    assert!(!dir.path().join("events.jsonl").exists());
}

#[test]
fn event_log_rotates_above_max() {
    let dir = tempfile::tempdir().unwrap();
    let mut envs = fixture_env(&dir);
    // tiny limit: the first run's events exceed it, so the second run
    // must rotate events.jsonl -> events.jsonl.1 before appending.
    envs.push(("SKILL_GRAPH_EVENT_LOG_MAX".to_string(), "10".to_string()));
    let payload = json!({"session_id":"r1","prompt":"alpha task"});
    let args = &["hook", "claude-code", "prompt"];
    run_hook(args, &payload, &env_pairs(&envs));
    run_hook(args, &payload, &env_pairs(&envs));
    let rotated = dir.path().join("events.jsonl.1");
    assert!(rotated.exists(), "rotated log missing: {:?}", dir.path());
    let rotated_events = event_lines(&rotated);
    assert!(!rotated_events.is_empty());
    // live log still receives the post-rotation events
    assert!(!event_lines(&dir.path().join("events.jsonl")).is_empty());
}

#[test]
fn unknown_runtime_and_mode_fail_open() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({"session_id":"u1","prompt":"alpha"});
    let (out, rc) = run_hook(
        &["hook", "not-a-runtime", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(out.trim().is_empty());
    let (out, rc) = run_hook(
        &["hook", "claude-code", "skill_load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(out.trim().is_empty());
}

#[test]
fn prompt_mode_records_recommended_names_per_session() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let state = dir.path().join("state");
    let payload = json!({"session_id":"s1","prompt":"use alpha-skill please"});
    let (out, rc) = run_hook(
        &["hook", "claude-code", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(!out.trim().is_empty());
    let rec_file = state.join("claude-code-s1.recommended");
    let names = std::fs::read_to_string(&rec_file).expect("recommended file");
    let lines: Vec<&str> = names.lines().collect();
    assert!(!lines.is_empty());
    // names only: every line is a fixture skill name, no prompt text
    for line in &lines {
        assert!(
            line.ends_with("-skill") || *line == "kappa-skill",
            "unexpected content: {line}"
        );
    }
    assert!(!names.contains("please"));
    // other sessions are untouched
    assert!(!state.join("claude-code-s2.recommended").exists());
}

#[test]
fn skill_load_uses_session_recommended_as_context() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    // Seed session context whose terms hit beta-skill's description
    // ("tests", "verification harnesses") — admits it via context alone.
    std::fs::write(state.join("claude-code-s1.recommended"), "tests\nharness\n").unwrap();
    let payload = json!({
        "session_id":"s1",
        "tool_name":"Skill",
        "tool_input":{"skill":"kappa-skill"}
    });
    let (out, rc) = run_hook(
        &["hook", "claude-code", "skill-load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    let v: Value = serde_json::from_str(out.trim()).expect("hook response json");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains("beta-skill"), "context was: {ctx}");
}

#[test]
fn skill_load_without_session_context_has_no_effect() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    // Same payload, no .recommended file -> kappa-skill has no deps,
    // so no dependency recommendation is emitted.
    let payload = json!({
        "session_id":"s9",
        "tool_name":"Skill",
        "tool_input":{"skill":"kappa-skill"}
    });
    let (out, rc) = run_hook(
        &["hook", "claude-code", "skill-load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(out.trim().is_empty());
    let events = event_lines(&dir.path().join("events.jsonl"));
    let rec = events
        .iter()
        .find(|e| e["kind"] == "recommendation")
        .expect("recommendation event");
    assert_eq!(rec["emitted"], false);
}

#[test]
fn nostate_records_no_session_recommendations() {
    let dir = tempfile::tempdir().unwrap();
    let mut envs = fixture_env(&dir);
    envs.push(("SKILL_GRAPH_HOOK_NOSTATE".to_string(), "1".to_string()));
    let payload = json!({"session_id":"s1","prompt":"use alpha-skill please"});
    let (_, rc) = run_hook(
        &["hook", "claude-code", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    assert!(
        !dir.path()
            .join("state")
            .join("claude-code-s1.recommended")
            .exists()
    );
}

#[test]
fn skill_load_accepts_name_key_tool_field() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    // OpenCode-style payloads may carry the tool name under `name`.
    let payload = json!({
        "session_id":"s1",
        "name":"Skill",
        "input":{"skill":"kappa-skill"}
    });
    let (out, rc) = run_hook(
        &["hook", "claude-code", "skill-load"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    let events = event_lines(&dir.path().join("events.jsonl"));
    assert!(
        events.iter().any(|e| e["kind"] == "skill_load"),
        "expected skill_load event, got: {events:?}"
    );
    let _ = out;
}

#[test]
fn non_object_stdin_writes_no_events() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    for raw in ["42", "\"x\"", "[1,2]", "true"] {
        let mut child = std::process::Command::new(bin)
            .args(["hook", "claude-code", "prompt"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(raw.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty());
    }
    assert!(!dir.path().join("events.jsonl").exists());
}

/// Every event line the hook actually writes must validate against
/// contracts/v1/skill-usage-event.schema.json — including non-emitted
/// recommendation events whose `recommended` list is empty (issue #144:
/// skip_serializing_if used to drop the key and observe then skipped
/// the lines, inflating the funnel delivery rate).
#[test]
fn emitted_events_validate_against_event_schema() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    let payload = json!({"session_id":"s1","prompt":"use alpha-skill please"});
    let args = &["hook", "claude-code", "prompt"];
    let (_, rc) = run_hook(args, &payload, &env_pairs(&envs));
    assert_eq!(rc, 0);
    // Second identical submit produces a dedup_skip recommendation with
    // recommended: [] — the exact shape that used to be schema-invalid.
    let (_, rc) = run_hook(args, &payload, &env_pairs(&envs));
    assert_eq!(rc, 0);

    let schema: Value = serde_json::from_str(include_str!(
        "../contracts/v1/skill-usage-event.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::options().build(&schema).unwrap();
    let events = event_lines(&dir.path().join("events.jsonl"));
    assert!(
        events.len() >= 3,
        "expected prompt_submit+2 recommendations: {events:?}"
    );
    let mut saw_empty_recommended = false;
    for (i, event) in events.iter().enumerate() {
        if event["kind"] == "recommendation" && event["recommended"] == json!([]) {
            saw_empty_recommended = true;
        }
        assert!(
            validator.is_valid(event),
            "event[{i}] failed schema: {}\nerrors: {:?}",
            serde_json::to_string_pretty(event).unwrap(),
            validator.iter_errors(event).collect::<Vec<_>>()
        );
    }
    assert!(saw_empty_recommended, "dedup_skip event missing from log");
}

/// Round-9: payload session ids that sanitize to empty must not
/// collapse distinct sessions into one `""` key — they get distinct
/// `unknown-<hash>` ids instead.
#[test]
fn degenerate_session_ids_stay_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let envs = fixture_env(&dir);
    for (i, raw) in ["!!!", "???", "日本語のみ"].iter().enumerate() {
        let payload = json!({"session_id": raw, "prompt": format!("alpha task {i}")});
        let (out, rc) = run_hook(
            &["hook", "claude-code", "prompt"],
            &payload,
            &env_pairs(&envs),
        );
        assert_eq!(rc, 0, "rc for {raw:?}: out={out}");
    }
    let events = event_lines(&dir.path().join("events.jsonl"));
    let ids: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "recommendation")
        .map(|e| e["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    for id in &ids {
        assert!(id.starts_with("unknown-"), "session_id {id}");
    }
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "degenerate ids must not collide: {ids:?}");
}

/// Round-13: env-supplied hook paths must be absolute — a relative
/// SKILL_GRAPH_EVENT_LOG/SKILL_GRAPH_HOOK_STATE/TMPDIR would resolve
/// against the hook's cwd (the project tree) and drop state there.
#[test]
fn relative_hook_env_paths_fall_back_to_tmpdir() {
    let tmp = tempfile::tempdir().unwrap();
    let envs: Vec<(String, String)> = vec![
        ("SKILL_GRAPH_ROOTS".to_string(), fixture_roots()),
        ("TMPDIR".to_string(), tmp.path().to_string_lossy().into()),
        (
            "SKILL_GRAPH_EVENT_LOG".to_string(),
            "relative-events.jsonl".to_string(),
        ),
        (
            "SKILL_GRAPH_HOOK_STATE".to_string(),
            "relative-state".to_string(),
        ),
    ];
    let payload = json!({"session_id":"s1","prompt":"alpha task"});
    let (_out, rc) = run_hook(
        &["hook", "claude-code", "prompt"],
        &payload,
        &env_pairs(&envs),
    );
    assert_eq!(rc, 0);
    // Relative values are ignored: the log lands under TMPDIR instead.
    assert!(tmp.path().join("skill-graph-events.jsonl").exists());
    assert!(!std::path::Path::new("relative-events.jsonl").exists());
}
