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
