//! Runtime hook entry point — Rust port of the v0.3.x
//! `skill-graph-recommend-hook` shell script.
//!
//! `skill-graph hook <runtime> <prompt|skill-load>` reads one hook payload
//! JSON on stdin, emits Skill Usage Events (contracts/v1) to the JSONL event
//! log, and prints the runtime hook response (additionalContext injection)
//! on stdout. Fails open: any internal error still exits 0.
//!
//! Env (same contract as the shell hook):
//!   SKILL_GRAPH_ROOTS        colon-separated skill roots
//!   SKILL_GRAPH_EVENT_LOG    JSONL event log
//!                            (default ${TMPDIR}/skill-graph-events.jsonl)
//!   SKILL_GRAPH_HOOK_STATE   dedup marker dir
//!                            (default ${TMPDIR}/skill-graph-hook-state)
//!   SKILL_GRAPH_EVENT_LOG_MAX rotate log above this many bytes (64 MiB)
//!   SKILL_GRAPH_HOOK_NOSTATE=1  no-save mode: no markers, no event writes

use crate::discovery::discover;
use crate::events::{
    self, Detection, ObservedVia, RecommendedSkill, Runtime, SkillUsageEvent, Trigger,
};
use crate::resolver::build;
use crate::search::recommend;
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;

const LOAD_GRADE: [Detection; 5] = [
    Detection::FileRead,
    Detection::PathReference,
    Detection::NativeInvocation,
    Detection::Inferred,
    Detection::Applied,
];

struct HookEnv {
    runtime: Runtime,
    nostate: bool,
    event_log: PathBuf,
    state_dir: PathBuf,
    event_log_max: u64,
    session_id: String,
    timestamp: String,
}

impl HookEnv {
    fn from_payload(runtime: Runtime, payload: &Value) -> Self {
        let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("sessionID"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let safe: String = session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
            .take(128)
            .collect();
        Self {
            runtime,
            nostate: std::env::var("SKILL_GRAPH_HOOK_NOSTATE").as_deref() == Ok("1"),
            event_log: std::env::var("SKILL_GRAPH_EVENT_LOG")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(&tmp).join("skill-graph-events.jsonl")),
            state_dir: std::env::var("SKILL_GRAPH_HOOK_STATE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(&tmp).join("skill-graph-hook-state")),
            event_log_max: std::env::var("SKILL_GRAPH_EVENT_LOG_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64 * 1024 * 1024),
            session_id: if safe.is_empty() {
                "unknown".to_string()
            } else {
                safe
            },
            timestamp: events::rfc3339_now(),
        }
    }

    fn log_event(&self, event: &SkillUsageEvent) {
        if self.nostate {
            return;
        }
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        if let Some(parent) = self.event_log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(meta) = std::fs::metadata(&self.event_log)
            && meta.len() > self.event_log_max
        {
            let _ = std::fs::rename(&self.event_log, self.event_log.with_extension("jsonl.1"));
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.event_log)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    fn marker_exists(&self, dedup_key: &str) -> bool {
        !self.nostate && self.marker_path(dedup_key).exists()
    }

    fn touch_marker(&self, dedup_key: &str) {
        if self.nostate {
            return;
        }
        let path = self.marker_path(dedup_key);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::File::create(path);
    }

    fn marker_path(&self, dedup_key: &str) -> PathBuf {
        // Marker names mirror the shell hook: <session>.<hash of dedup_key>.
        // The hash is internal-only (no cross-version compat requirement).
        let key_hash = events::sha256_hex(dedup_key.as_bytes());
        self.state_dir
            .join(format!("{}.{key_hash}", self.session_id))
    }
}

fn hook_roots(runtime: Runtime) -> Vec<PathBuf> {
    if let Ok(spec) = std::env::var("SKILL_GRAPH_ROOTS") {
        return spec
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dot_dir = match runtime {
        Runtime::Codex => ".codex",
        _ => ".claude",
    };
    let mut roots = Vec::new();
    if let Some(h) = &home {
        roots.push(h.join(format!("{dot_dir}/skills")));
        roots.push(h.join(".agents/skills"));
    }
    roots.push(cwd.join("skills"));
    roots.push(cwd.join(format!("{dot_dir}/skills")));
    roots.push(cwd.join(".agents/skills"));
    roots.into_iter().filter(|p| p.is_dir()).collect()
}

fn recommended_from(recs: &[crate::model::Recommendation]) -> Vec<RecommendedSkill> {
    recs.iter()
        .map(|r| RecommendedSkill {
            name: r.skill.name.clone(),
            digest: Some(r.skill.digest.clone()),
        })
        .collect()
}

fn recommend_names(
    query: &str,
    max: usize,
    roots: &[PathBuf],
) -> Result<Vec<crate::model::Recommendation>, String> {
    let graph = build(discover(roots));
    Ok(recommend(&graph, query, "", max))
}

fn emit_recommendation(
    env: &HookEnv,
    trigger: Trigger,
    prompt_hash: Option<String>,
    recs: &[crate::model::Recommendation],
    emitted: bool,
    reason: &str,
    recommend_ms: Option<u64>,
) {
    env.log_event(&SkillUsageEvent::recommendation(
        env.runtime,
        ObservedVia::Hook,
        &env.session_id,
        &env.timestamp,
        prompt_hash,
        trigger,
        recommended_from(recs),
        emitted,
        reason,
        recommend_ms,
    ));
}

fn hook_response(event_name: &str, context: &str) -> String {
    json!({"hookSpecificOutput": {"hookEventName": event_name, "additionalContext": context}})
        .to_string()
}

/// Cheap pre-filter mirroring the shell hook: only Skill-tool calls or
/// tool inputs referencing SKILL.md reach detection/recommendation.
fn skill_load_prefilter(payload: &Value) -> bool {
    let tool = payload
        .get("tool_name")
        .or_else(|| payload.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool == "Skill" {
        return true;
    }
    let input = payload
        .get("tool_input")
        .or_else(|| payload.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    if input.is_null() {
        return false;
    }
    let text = match &input {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    text.contains("SKILL.md")
}

fn run_skill_load(env: &HookEnv, payload: &Value) -> Option<String> {
    if !skill_load_prefilter(payload) {
        return None;
    }
    let event = events::detect_from_payload(env.runtime, ObservedVia::Hook, payload)?;
    env.log_event(&event);
    let detection = event.detection?;
    if !LOAD_GRADE.contains(&detection) {
        return None;
    }
    let skill_name = event.skill_name.clone()?;

    let dedup_key = format!("skill:{skill_name}");
    if env.marker_exists(&dedup_key) {
        emit_recommendation(
            env,
            Trigger::SkillLoad,
            None,
            &[],
            false,
            "dedup_skip",
            Some(0),
        );
        return None;
    }
    env.touch_marker(&dedup_key);

    let roots = hook_roots(env.runtime);
    if roots.is_empty() {
        emit_recommendation(
            env,
            Trigger::SkillLoad,
            None,
            &[],
            false,
            "no_roots",
            Some(0),
        );
        return None;
    }

    let start = Instant::now();
    let recs = match recommend_names(&skill_name, 5, &roots) {
        Ok(r) => r,
        Err(_) => {
            emit_recommendation(
                env,
                Trigger::SkillLoad,
                None,
                &[],
                false,
                "recommender_error",
                Some(0),
            );
            return None;
        }
    };
    let recommend_ms = start.elapsed().as_millis() as u64;

    let names: Vec<&str> = recs
        .iter()
        .map(|r| r.skill.name.as_str())
        .filter(|n| *n != skill_name.as_str())
        .take(3)
        .collect();
    if names.is_empty() {
        emit_recommendation(
            env,
            Trigger::SkillLoad,
            None,
            &recs,
            false,
            "no_recommendations",
            Some(recommend_ms),
        );
        return None;
    }
    emit_recommendation(
        env,
        Trigger::SkillLoad,
        None,
        &recs,
        true,
        "recommended",
        Some(recommend_ms),
    );
    let context = format!(
        "skill-graph: '{skill_name}' declares dependencies — {}. Read the matching SKILL.md files if the task needs them.",
        names.join(", ")
    );
    Some(hook_response("PostToolUse", &context))
}

fn run_prompt(env: &HookEnv, payload: &Value) -> Option<String> {
    let query = payload
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !query.is_empty() {
        env.log_event(&SkillUsageEvent::prompt_submit(
            env.runtime,
            ObservedVia::Hook,
            &env.session_id,
            &env.timestamp,
            &query,
        ));
    }
    let phash = events::prompt_hash(&query);
    let roots = hook_roots(env.runtime);
    if query.is_empty() || roots.is_empty() {
        emit_recommendation(
            env,
            Trigger::PromptSubmit,
            Some(phash),
            &[],
            false,
            "no_input_or_roots",
            Some(0),
        );
        return None;
    }

    let dedup_key = format!("prompt:{}", events::sha256_hex(query.as_bytes()));
    if env.marker_exists(&dedup_key) {
        emit_recommendation(
            env,
            Trigger::PromptSubmit,
            Some(phash),
            &[],
            false,
            "dedup_skip",
            Some(0),
        );
        return None;
    }
    env.touch_marker(&dedup_key);

    let start = Instant::now();
    let recs = match recommend_names(&query, 5, &roots) {
        Ok(r) => r,
        Err(_) => {
            emit_recommendation(
                env,
                Trigger::PromptSubmit,
                Some(phash),
                &[],
                false,
                "recommender_error",
                Some(0),
            );
            return None;
        }
    };
    let recommend_ms = start.elapsed().as_millis() as u64;

    let names: Vec<&str> = recs.iter().take(3).map(|r| r.skill.name.as_str()).collect();
    if names.is_empty() {
        emit_recommendation(
            env,
            Trigger::PromptSubmit,
            Some(phash.clone()),
            &recs,
            false,
            "no_recommendations",
            Some(recommend_ms),
        );
        return None;
    }
    emit_recommendation(
        env,
        Trigger::PromptSubmit,
        Some(phash),
        &recs,
        true,
        "recommended",
        Some(recommend_ms),
    );
    let context = format!(
        "skill-graph: skills relevant to this task — {}. Read the matching SKILL.md before relying on it.",
        names.join(", ")
    );
    Some(hook_response("UserPromptSubmit", &context))
}

/// Entry point for `skill-graph hook`. Always exits 0 (fail-open); prints
/// the hook response JSON only when there is context to inject.
pub fn run_hook(runtime: Runtime, mode: &str) -> Result<(), String> {
    let _ = run_hook_inner(runtime, mode);
    Ok(())
}

fn run_hook_inner(runtime: Runtime, mode: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("stdin: {e}"))?;
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let env = HookEnv::from_payload(runtime, &payload);

    if env.nostate {
        // No state dir needed, but the event log is also skipped by log_event.
    } else {
        let _ = std::fs::create_dir_all(&env.state_dir);
    }

    let response = match mode {
        "skill-load" => run_skill_load(&env, &payload),
        _ => run_prompt(&env, &payload),
    };
    if let Some(out) = response {
        println!("{out}");
    }
    Ok(())
}
