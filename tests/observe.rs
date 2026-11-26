use skill_graph::events::EventKind;
use skill_graph::observe::observe_path;
use std::fs;
use tempfile::TempDir;

fn write_transcript(dir: &TempDir, name: &str, lines: &[&str]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

#[test]
fn counts_skill_tool_use_per_session() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "a.jsonl",
        &[
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"ns:alpha-skill"}}]}}"#,
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-01-01T00:01:00Z","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","sessionId":"s2","timestamp":"2026-01-01T00:02:00Z","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"beta-skill"}},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            "not json",
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert_eq!(obs.files_scanned, 1);
    assert_eq!(obs.sessions, 2);
    assert_eq!(obs.lines_skipped, 1);
    assert_eq!(obs.skill_loads.get("alpha-skill"), Some(&1));
    assert_eq!(obs.skill_loads.get("beta-skill"), Some(&1));
    assert_eq!(obs.loads.len(), 2);
    let skill_event = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::SkillLoad && e.skill_name.as_deref() == Some("alpha-skill"))
        .expect("skill_load event");
    assert_eq!(
        skill_event
            .detection
            .map(|d| serde_json::to_value(d).unwrap()),
        Some(serde_json::json!("native_invocation"))
    );
}

#[test]
fn empty_and_unrelated_transcripts_produce_zero_loads() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "b.jsonl",
        &[
            r#"{"type":"assistant","sessionId":"s1","message":{"content":[{"type":"tool_use","name":"Read","input":{"path":"x"}}]}}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert!(obs.skill_loads.is_empty());
    assert_eq!(obs.sessions, 1);
}

#[test]
fn claude_file_read_is_file_read_with_tool_result() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "c.jsonl",
        &[
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"path":"/repo/.agents/skills/alpha-skill/SKILL.md"}}]}}"#,
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"---\nname: alpha-skill\ndescription: x\n---\nbody"}]}}"#,
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"grep foo .agents/skills/beta/SKILL.md"}}]}}"#,
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"no match"}]}}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    let loads: Vec<_> = obs
        .events
        .iter()
        .filter(|e| e.kind == EventKind::SkillLoad)
        .collect();
    assert_eq!(loads.len(), 2);
    let read = loads
        .iter()
        .find(|e| e.skill_name.as_deref() == Some("alpha-skill"))
        .unwrap();
    assert_eq!(
        read.detection.map(|d| serde_json::to_value(d).unwrap()),
        Some(serde_json::json!("file_read"))
    );
    assert_eq!(
        read.confidence.map(|c| serde_json::to_value(c).unwrap()),
        Some(serde_json::json!("high"))
    );
    let mention = loads
        .iter()
        .find(|e| e.skill_name.as_deref() == Some("beta"))
        .unwrap();
    assert_eq!(
        mention.detection.map(|d| serde_json::to_value(d).unwrap()),
        Some(serde_json::json!("mention"))
    );
    // mention does not count as a load
    assert_eq!(obs.skill_loads.get("beta"), None);
    assert_eq!(obs.skill_loads.get("alpha-skill"), Some(&1));
}

#[test]
fn codex_rollout_normalizes_exec_loads_and_injection() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "rollout-x.jsonl",
        &[
            r#"{"timestamp":"2026-11-26T01:12:30Z","type":"session_meta","payload":{"session_id":"sess-1","id":"sess-1","cwd":"/tmp/p"}}"#,
            r#"{"timestamp":"2026-11-26T01:12:31Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Read the file .agents/skills/alpha-skill/SKILL.md"}]}}"#,
            r#"{"timestamp":"2026-11-26T01:12:32Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call_1","input":"text(await tools.exec_command({cmd:\"cat .agents/skills/alpha-skill/SKILL.md\"}))\n"}}"#,
            r#"{"timestamp":"2026-11-26T01:12:33Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_1","output":[{"type":"input_text","text":"Script completed\nOutput:\n"},{"type":"input_text","text":"{\"chunk_id\":\"0e47b4\",\"exit_code\":0,\"output\":\"---\\nname: alpha-skill\\ndescription: d\\n---\\nbody\\n\"}"}]}}"#,
            r#"{"timestamp":"2026-11-26T01:12:34Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"skill-graph: skills relevant to this task — beta-skill. Read the matching SKILL.md before relying on it."}]}}"#,
            r#"{"timestamp":"2026-11-26T01:12:35Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call_2","input":"text(await tools.exec_command({cmd:\"wc -l .agents/skills/gamma/SKILL.md\"}))\n"}}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert!(obs.errors.is_empty(), "errors: {:?}", obs.errors);

    let prompt = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::PromptSubmit)
        .expect("prompt_submit");
    assert_eq!(prompt.runtime.as_str(), "codex");
    assert!(prompt.prompt_hash.is_some());

    let load = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::SkillLoad && e.skill_name.as_deref() == Some("alpha-skill"))
        .expect("skill_load");
    assert_eq!(
        load.detection.map(|d| serde_json::to_value(d).unwrap()),
        Some(serde_json::json!("file_read"))
    );
    assert_eq!(
        load.observed_via,
        skill_graph::events::ObservedVia::Transcript
    );

    let mention = obs
        .events
        .iter()
        .find(|e| {
            e.kind == EventKind::SkillLoad
                && e.skill_name.as_deref() == Some("gamma")
                && e.detection.map(|d| serde_json::to_value(d).unwrap())
                    == Some(serde_json::json!("mention"))
        })
        .expect("mention-level event");
    assert_eq!(
        mention.confidence.map(|c| serde_json::to_value(c).unwrap()),
        Some(serde_json::json!("low"))
    );

    let rec = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::Recommendation)
        .expect("recommendation from injected context");
    assert_eq!(rec.recommended[0].name, "beta-skill");
    assert_eq!(
        rec.trigger.map(|t| serde_json::to_value(t).unwrap()),
        Some(serde_json::json!("prompt_submit"))
    );
    // mention does not count as a load
    assert_eq!(obs.runtimes.get("codex"), Some(&1));
    assert_eq!(obs.skill_loads.get("gamma"), None);
}

#[test]
fn contract_event_log_passes_through() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "events.jsonl",
        &[
            r#"{"schema_version":1,"kind":"prompt_submit","runtime":"codex","session_id":"s1","timestamp":"2026-11-26T00:00:00Z","observed_via":"hook","prompt_hash":"abc123"}"#,
            r#"{"schema_version":1,"kind":"skill_load","runtime":"codex","session_id":"s1","timestamp":"2026-11-26T00:00:01Z","observed_via":"hook","skill_name":"alpha-skill","detection":"file_read","confidence":"high","tool":"Bash"}"#,
            r#"{"schema_version":2,"kind":"skill_load","runtime":"codex","session_id":"s1","observed_via":"hook","skill_name":"x","detection":"file_read","confidence":"high"}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert_eq!(obs.events.len(), 2);
    assert_eq!(obs.lines_skipped, 1);
    assert_eq!(obs.skill_loads.get("alpha-skill"), Some(&1));
    assert_eq!(obs.runtimes.get("codex"), Some(&1));
}

#[test]
fn opencode_db_normalizes_messages() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message VALUES ('m1','ses_1','user',1,1793321018917,1793321018917,?)",
        [
            serde_json::json!({"time":{"created":1793321018917i64},"text":"please review this"})
                .to_string(),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message VALUES ('m2','ses_1','assistant',2,1793321019000,1793321019000,?)",
        [serde_json::json!({"time":{"created":1793321019000i64},"content":[
            {"type":"tool","name":"read","state":{"status":"completed","input":{"path":"/repo/.opencode/skill/probe/SKILL.md"},"content":[{"type":"text","text":"---\nname: probe-skill\n---\nbody"}]}}
        ]}).to_string()],
    )
    .unwrap();
    drop(conn);

    let obs = observe_path(dir.path()).unwrap();
    assert!(obs.errors.is_empty(), "errors: {:?}", obs.errors);
    let prompt = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::PromptSubmit)
        .expect("prompt_submit");
    assert_eq!(prompt.runtime.as_str(), "opencode");
    let load = obs
        .events
        .iter()
        .find(|e| e.kind == EventKind::SkillLoad)
        .expect("skill_load");
    assert_eq!(load.skill_name.as_deref(), Some("probe-skill"));
    assert_eq!(
        load.detection.map(|d| serde_json::to_value(d).unwrap()),
        Some(serde_json::json!("file_read"))
    );
    assert_eq!(obs.runtimes.get("opencode"), Some(&1));
}

fn rec_event(
    session: &str,
    ts: &str,
    names: &[&str],
    emitted: bool,
) -> skill_graph::events::SkillUsageEvent {
    use skill_graph::events::{ObservedVia, RecommendedSkill, Runtime, Trigger};
    skill_graph::events::SkillUsageEvent::recommendation(
        Runtime::ClaudeCode,
        ObservedVia::Hook,
        session,
        ts,
        None,
        Trigger::PromptSubmit,
        names
            .iter()
            .map(|n| RecommendedSkill {
                name: n.to_string(),
                digest: None,
            })
            .collect(),
        emitted,
        "test",
        None,
    )
}

fn load_event(session: &str, ts: &str, name: &str) -> skill_graph::events::SkillUsageEvent {
    use skill_graph::events::{Confidence, Detection, ObservedVia, Runtime};
    skill_graph::events::SkillUsageEvent::skill_load(
        Runtime::ClaudeCode,
        ObservedVia::Hook,
        session,
        ts,
        None,
        Some(name.to_string()),
        None,
        Detection::FileRead,
        Confidence::High,
    )
}

#[test]
fn summary_computes_funnel_metrics() {
    use skill_graph::events::Runtime;
    use skill_graph::observe::{Observation, summary};

    // s1: emitted rec recommends alpha+beta; alpha loads 60s later
    //     (converted), beta never loads; gamma loads unprompted
    //     (unnecessary). s2: non-emitted rec; delta-skill loads.
    let obs = Observation {
        events: vec![
            rec_event("s1", "2026-01-01T00:00:00Z", &["alpha", "beta"], true),
            rec_event("s1", "2026-01-01T00:10:00Z", &["beta"], false),
            rec_event("s2", "2026-01-01T00:00:00Z", &["delta"], true),
            load_event("s1", "2026-01-01T00:01:00Z", "alpha"),
            load_event("s1", "2026-01-01T00:02:00Z", "gamma"),
            load_event("s2", "2026-01-01T00:00:30Z", "delta"),
        ],
        ..Default::default()
    };
    let v = summary(&obs);
    assert_eq!(v["recommendations"], 3);
    assert_eq!(v["emitted_recommendations"], 2);
    assert_eq!(v["delivery_rate_pct"], 66.7);
    assert_eq!(v["load_grade_loads"], 3);
    assert_eq!(v["converted_recommendations"], 2);
    assert_eq!(v["conversion_rate_pct"], 100.0);
    // gamma was never recommended -> unnecessary; alpha/delta converted
    assert_eq!(v["unnecessary_loads"], 1);
    assert_eq!(v["unnecessary_load_rate_pct"], 33.3);
    assert_eq!(v["timeliness_ms"]["count"], 2);
    assert_eq!(v["timeliness_ms"]["p50"], 60000);
    assert_eq!(v["timeliness_ms"]["max"], 60000);
    assert_eq!(v["sessions"], 2);
    assert!(v["by_runtime"]["claude-code"].is_object());

    // events of other runtimes land in their own bucket only
    let mut e = load_event("s3", "2026-01-01T00:00:00Z", "alpha");
    e.runtime = Runtime::Codex;
    let obs2 = Observation {
        events: vec![e],
        ..Default::default()
    };
    let v2 = summary(&obs2);
    assert_eq!(v2["load_grade_loads"], 1);
    assert_eq!(v2["unnecessary_loads"], 1);
    assert_eq!(v2["by_runtime"]["codex"]["load_grade_loads"], 1);
    assert!(v2["by_runtime"].get("claude-code").is_none());
}

/// Session ids are only unique within a runtime: the same id under
/// claude-code and codex must not merge into one funnel session, and
/// events with an empty session id count toward top-level counters but
/// never join a session join (conversion/unnecessary).
#[test]
fn summary_isolates_sessions_across_runtimes() {
    use skill_graph::events::Runtime;
    use skill_graph::observe::{Observation, summary};

    // claude-code "s1": emitted rec for alpha, then alpha loads -> converted.
    // codex "s1": same ids but a load of beta with NO prior recommendation
    // in that session -> unnecessary, and must not convert claude's rec.
    let mut codex_load = load_event("s1", "2026-01-01T00:01:30Z", "beta");
    codex_load.runtime = Runtime::Codex;
    let obs = Observation {
        events: vec![
            rec_event("s1", "2026-01-01T00:00:00Z", &["alpha"], true),
            load_event("s1", "2026-01-01T00:01:00Z", "alpha"),
            codex_load,
        ],
        ..Default::default()
    };
    let v = summary(&obs);
    assert_eq!(v["sessions"], 2, "runtime:session pairs are distinct");
    assert_eq!(v["converted_recommendations"], 1);
    assert_eq!(v["unnecessary_loads"], 1);
    assert_eq!(
        v["by_session"]["claude-code:s1"]["converted_recommendations"],
        1
    );
    assert_eq!(v["by_session"]["codex:s1"]["unnecessary_loads"], 1);
    assert_eq!(v["by_session"]["codex:s1"]["converted_recommendations"], 0);

    // Empty session id: still counted at top level, absent from by_session.
    let mut no_sid = load_event("", "2026-01-01T00:02:00Z", "gamma");
    no_sid.session_id = String::new();
    let obs2 = Observation {
        events: vec![no_sid],
        ..Default::default()
    };
    let v2 = summary(&obs2);
    assert_eq!(v2["load_grade_loads"], 1);
    assert_eq!(v2["sessions"], 0);
    assert_eq!(v2["unnecessary_loads"], 0, "no session -> no join");
    assert_eq!(v2["by_session"].as_object().unwrap().len(), 0);
}

#[cfg(unix)]
#[test]
fn observe_survives_symlink_cycle() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "a.jsonl",
        &[
            r#"{"type":"user","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}"#,
        ],
    );
    // loop -> . creates a directory cycle; must not recurse forever
    std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
    let obs = observe_path(dir.path()).unwrap();
    assert_eq!(obs.files_scanned, 1);
}

/// summary() must surface collection errors and by_session truncation
/// rather than returning partial aggregates indistinguishable from clean
/// ones.
#[test]
fn summary_reports_errors_and_session_cap() {
    use skill_graph::observe::{Observation, summary};

    let mut obs = Observation {
        errors: (0..105).map(|i| format!("err-{i}")).collect(),
        ..Default::default()
    };
    // 501 distinct sessions exceed MAX_SUMMARY_SESSIONS (500).
    for i in 0..501 {
        obs.events.push(load_event(
            &format!("sess-{i}"),
            "2026-01-01T00:00:00Z",
            "alpha",
        ));
    }
    let v = summary(&obs);
    assert_eq!(v["collection_errors"], 105);
    assert_eq!(v["errors"].as_array().unwrap().len(), 100);
    assert_eq!(v["errors_omitted"], 5);
    assert_eq!(v["by_session"].as_object().unwrap().len(), 500);
    assert_eq!(v["by_session_omitted"], 1);
}

/// Passthrough events are untrusted log input: session_id is normalized
/// to the constructor bounds, oversized fields are capped, and a
/// skill_load with an empty skill_name is counted in lines_skipped
/// rather than vanishing silently.
#[test]
fn passthrough_normalizes_fields_and_counts_empty_name() {
    let dir = TempDir::new().unwrap();
    let long_sid = "a".repeat(200);
    let event = format!(
        r#"{{"schema_version":1,"kind":"skill_load","runtime":"claude-code","session_id":"{long_sid}","timestamp":"2026-01-01T00:00:00Z","observed_via":"hook","skill_name":"alpha","detection":"file_read","confidence":"high"}}"#
    );
    write_transcript(
        &dir,
        "events.jsonl",
        &[
            event.as_str(),
            // empty skill_name: present but unattributable -> skipped
            r#"{"schema_version":1,"kind":"skill_load","runtime":"claude-code","session_id":"s1","timestamp":"2026-01-01T00:00:01Z","observed_via":"hook","skill_name":"","detection":"file_read","confidence":"high"}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert_eq!(obs.events.len(), 1);
    assert_eq!(obs.lines_skipped, 1);
    assert_eq!(obs.events[0].session_id.len(), 128);
    assert!(
        obs.events[0]
            .session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    );
}

/// Recommendation events with an empty `recommended` list must pass
/// through (the hook emits `recommended: []` for dedup/no_recommendations
/// outcomes), while explicit JSON nulls for required fields are rejected
/// rather than re-serialized into schema-invalid output.
#[test]
fn passthrough_accepts_empty_recommended_rejects_null_required() {
    let dir = TempDir::new().unwrap();
    write_transcript(
        &dir,
        "events.jsonl",
        &[
            // valid: emitted=false with recommended: []
            r#"{"schema_version":1,"kind":"recommendation","runtime":"codex","session_id":"s1","timestamp":"2026-11-26T00:00:00Z","observed_via":"hook","trigger":"prompt_submit","recommended":[],"emitted":false,"reason":"dedup_skip"}"#,
            // invalid: emitted is explicit null — must be skipped
            r#"{"schema_version":1,"kind":"recommendation","runtime":"codex","session_id":"s1","timestamp":"2026-11-26T00:00:01Z","observed_via":"hook","trigger":"prompt_submit","recommended":[],"emitted":null,"reason":"x"}"#,
            // invalid: recommended item name empty
            r#"{"schema_version":1,"kind":"recommendation","runtime":"codex","session_id":"s1","timestamp":"2026-11-26T00:00:02Z","observed_via":"hook","trigger":"prompt_submit","recommended":[{"name":""}],"emitted":true,"reason":"recommended"}"#,
        ],
    );
    let obs = observe_path(dir.path()).unwrap();
    assert_eq!(obs.events.len(), 1, "events: {:?}", obs.events);
    assert_eq!(obs.lines_skipped, 2);
    assert_eq!(
        obs.events[0].kind,
        skill_graph::events::EventKind::Recommendation
    );
    assert_eq!(obs.events[0].emitted, Some(false));
}
