//! Read-only observation of existing agent session transcripts.
//!
//! Parses Claude Code JSONL transcripts (`~/.claude/projects/**`) for `Skill`
//! tool invocations. Other runtimes are tolerated (unknown line shapes are
//! skipped) but only the Claude Code `tool_use`/`Skill` shape is counted.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize)]
pub struct Observation {
    pub files_scanned: usize,
    pub lines_skipped: usize,
    pub sessions: usize,
    pub skill_loads: BTreeMap<String, usize>,
    pub loads: Vec<SkillLoad>,
}

#[derive(Debug, Serialize)]
pub struct SkillLoad {
    pub session_id: String,
    pub skill: String,
    pub timestamp: String,
    pub file: String,
}

pub fn observe_path(path: &Path) -> Result<Observation, String> {
    let mut files = Vec::new();
    collect_jsonl(path, &mut files)?;
    files.sort();
    let mut obs = Observation::default();
    let mut sessions = std::collections::BTreeSet::new();
    for file in files {
        obs.files_scanned += 1;
        let content = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                obs.lines_skipped += 1;
                continue;
            };
            let Some(session_id) = value.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            sessions.insert(session_id.to_string());
            let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for item in content {
                if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                if item.get("name").and_then(Value::as_str) != Some("Skill") {
                    continue;
                }
                let Some(skill) = item
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let skill = skill.rsplit(['/', ':']).next().unwrap_or(skill).to_string();
                *obs.skill_loads.entry(skill.clone()).or_default() += 1;
                obs.loads.push(SkillLoad {
                    session_id: session_id.to_string(),
                    skill,
                    timestamp: value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    file: file.display().to_string(),
                });
            }
        }
    }
    obs.sessions = sessions.len();
    Ok(obs)
}

fn collect_jsonl(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("not a file or directory: {}", path.display()));
    }
    for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if p.is_dir() {
            collect_jsonl(&p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
    Ok(())
}
