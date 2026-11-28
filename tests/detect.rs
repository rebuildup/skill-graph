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

/// The detect boundary normalizes the RAW payload session id exactly
/// once: output must equal SessionId::normalize(input) — the same value
/// the OpenCode plugin's safeSessionID produces for its own state keys
/// (#225 r7). This is the cross-language contract made executable: the
/// real binary, not a reimplementation.
#[test]
fn detect_normalizes_raw_session_id_once() {
    let long_id = "z".repeat(200);
    for raw in [
        "sess!1",                 // stripped char → transform
        long_id.as_str(),         // over-length → transform
        "sess 1\u{1f600}unicode", // non-ASCII → transform
        "plain-ok_1.2",           // canonical → passthrough
        "",                       // absent-equivalent
    ] {
        let mut payload = skill_read_payload();
        payload["session_id"] = json!(raw);
        let (out, rc) = run_detect(
            &["detect", "--runtime", "claude-code", "--via", "hook"],
            &payload.to_string(),
        );
        assert_eq!(rc, 0, "raw={raw:?}: {out}");
        let event: Value = serde_json::from_str(out.trim()).expect("event json");
        let expected = skill_graph::events::SessionId::normalize(raw).to_string();
        assert_eq!(
            event["session_id"].as_str().unwrap(),
            expected,
            "raw={raw:?}"
        );
        // Normalized contract form: bounded length, safe charset.
        let sid = event["session_id"].as_str().unwrap();
        assert!(sid.chars().count() <= 128, "sid too long: {sid:?}");
        assert!(
            sid.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "sid charset violation: {sid:?}"
        );
    }
}

/// A payload carrying an already-normalized (reserved-shape) id is
/// still RAW input at this boundary: detect re-transforms it rather
/// than passing it through — non-idempotency is the documented
/// contract, and producers must send raw ids (#225 r7).
#[test]
fn detect_treats_reserved_shape_input_as_raw() {
    let reserved = format!("sess-{}", "a".repeat(32)); // matches -<32hex>
    let mut payload = skill_read_payload();
    payload["session_id"] = json!(reserved);
    let (out, rc) = run_detect(
        &["detect", "--runtime", "claude-code", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let event: Value = serde_json::from_str(out.trim()).expect("event json");
    let sid = event["session_id"].as_str().unwrap();
    assert_ne!(sid, reserved, "reserved-shape input must be re-transformed");
    assert_eq!(
        sid,
        skill_graph::events::SessionId::normalize(&reserved).to_string()
    );
}

/// A command touching several SKILL.md files must emit one NDJSON event
/// per file — the OpenCode plugin depends on this line-per-event shape.
#[test]
fn detect_emits_one_line_per_skill_md_access() {
    let payload = json!({
        "tool_name": "Bash",
        "tool_input": {"command": "cat /repo/skills/alpha-skill/SKILL.md; grep -r foo /repo/skills/beta-skill/SKILL.md"},
        "tool_response": {"stdout": "..."},
        "session_id": "s-multi"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "claude-code", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 NDJSON events: {out}");
    let events: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("ndjson line"))
        .collect();
    let names: Vec<&str> = events
        .iter()
        .filter_map(|e| e["skill_name"].as_str())
        .collect();
    assert!(names.contains(&"alpha-skill"), "{names:?}");
    assert!(names.contains(&"beta-skill"), "{names:?}");
}

/// A payload touching more SKILL.md files than MAX_EVENTS_PER_PAYLOAD
/// (64) must emit exactly 64 NDJSON lines — the cap bounds per-payload
/// work for both the hook and the OpenCode plugin.
#[test]
fn detect_truncates_events_per_payload() {
    let cmd = (0..70)
        .map(|i| format!("cat /repo/skills/skill-{i:03}/SKILL.md"))
        .collect::<Vec<_>>()
        .join(";");
    let payload = json!({
        "tool_name": "Bash",
        "tool_input": {"command": cmd},
        "tool_response": {"stdout": "..."},
        "session_id": "s-cap"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "claude-code", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        64,
        "expected capped 64 events, got {}",
        lines.len()
    );
}

/// Codex `exec` tool carries JS source, not a shell command; the
/// exec_command({cmd: ...}) extraction path must classify SKILL.md
/// accesses inside it.
#[test]
fn detect_exec_tool_extracts_js_commands() {
    let payload = json!({
        "tool_name": "exec",
        "tool_input": {"input": "await exec_command({cmd: \"cat /repo/skills/alpha-skill/SKILL.md\"})"},
        "tool_response": "file contents",
        "session_id": "s-exec"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "codex", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let event: Value = serde_json::from_str(out.trim()).expect("event json");
    assert_eq!(event["kind"], "skill_load");
    assert_eq!(event["skill_name"], "alpha-skill");
}

/// Exec-branch dedup keys on skill identity and keeps the strongest
/// classification: `grep` (mention) before `cat` (file_read) on the same
/// SKILL.md must not shadow the verified read — and one path must not
/// emit two events under different derived names.
#[test]
fn detect_exec_dedup_keeps_strongest_classification() {
    let payload = json!({
        "tool_name": "exec",
        "tool_input": {"input": "await exec_command({cmd: \"grep foo /repo/skills/x-skill/SKILL.md\"})\nawait exec_command({cmd: \"cat /repo/skills/x-skill/SKILL.md\"})"},
        "tool_response": "---\nname: x-real\n---\nbody",
        "session_id": "s-exec-dedup"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "codex", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "expected one deduped event: {out}");
    let event: Value = serde_json::from_str(lines[0]).expect("event json");
    assert_eq!(event["detection"], "file_read");
    assert_eq!(event["skill_name"], "x-real");
}

/// One tool output covering several SKILL.md reads: the frontmatter
/// block belongs to the first file emitted — later reads must fall back
/// to path-derived classification instead of inheriting the wrong name.
#[test]
fn detect_multi_skill_md_output_attributes_frontmatter_to_first_only() {
    let payload = json!({
        "tool_name": "Bash",
        "tool_input": {"command": "cat /repo/skills/alpha/SKILL.md /repo/skills/beta/SKILL.md"},
        "tool_response": "---\nname: alpha-real\n---\nalpha body\n---\nname: beta-real\n---\nbeta body",
        "session_id": "s-multi"
    });
    let (out, rc) = run_detect(
        &["detect", "--runtime", "claude-code", "--via", "hook"],
        &payload.to_string(),
    );
    assert_eq!(rc, 0, "{out}");
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected two events: {out}");
    let events: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("event json"))
        .collect();
    let first = events
        .iter()
        .find(|e| e["skill_path"] == "/repo/skills/alpha/SKILL.md")
        .expect("alpha event");
    assert_eq!(first["skill_name"], "alpha-real");
    assert_eq!(first["detection"], "file_read");
    let second = events
        .iter()
        .find(|e| e["skill_path"] == "/repo/skills/beta/SKILL.md")
        .expect("beta event");
    assert_eq!(second["skill_name"], "beta");
    assert_eq!(second["detection"], "path_reference");
}
