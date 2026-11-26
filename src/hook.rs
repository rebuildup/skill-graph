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
            // v0.3.x convention: append ".1" to the full log path.
            let rotated = PathBuf::from(format!("{}.1", self.event_log.display()));
            let _ = std::fs::rename(&self.event_log, rotated);
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
        // Marker names: <session>.<cksum(dedup_key)>, identical to the
        // v0.3.x shell hook so dedup state survives migration.
        let key_hash = cksum_decimal(dedup_key.as_bytes());
        self.state_dir
            .join(format!("{}.{key_hash}", self.session_id))
    }

    /// Per-session record of skill names surfaced by prompt-mode
    /// recommendations (issue #128). Names only — prompt text, commands,
    /// and tool responses are never persisted here.
    fn recommended_path(&self) -> PathBuf {
        self.state_dir
            .join(format!("{}.recommended", self.session_id))
    }

    fn session_recommended(&self) -> Vec<String> {
        if self.nostate {
            return Vec::new();
        }
        std::fs::read_to_string(self.recommended_path())
            .map(|s| {
                s.lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn record_recommended(&self, names: &[String]) {
        if self.nostate || names.is_empty() {
            return;
        }
        const CAP: usize = 32;
        let mut all = self.session_recommended();
        for n in names {
            if !all.contains(n) {
                all.push(n.clone());
            }
        }
        if all.len() > CAP {
            all.drain(..all.len() - CAP);
        }
        if let Some(parent) = self.recommended_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(self.recommended_path(), all.join("\n") + "\n");
    }
}

/// POSIX cksum (the algorithm the `cksum` command implements): MSB-first
/// CRC, polynomial 0x04C11DB7, init 0; the input length is appended
/// least-significant-byte first; the result is XORed with 0xFFFFFFFF and
/// printed as decimal. Verified byte-for-byte against `cksum(1)`.
fn cksum_decimal(data: &[u8]) -> String {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut crc = (i as u32) << 24;
            let mut j = 0;
            while j < 8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
                j += 1;
            }
            t[i] = crc;
            i += 1;
        }
        t
    };
    let mut crc = 0u32;
    for &b in data {
        crc = TABLE[(((crc >> 24) as u8) ^ b) as usize] ^ (crc << 8);
    }
    let mut n = data.len() as u64;
    while n > 0 {
        crc = TABLE[(((crc >> 24) as u8) ^ (n & 0xff) as u8) as usize] ^ (crc << 8);
        n >>= 8;
    }
    (crc ^ 0xffff_ffff).to_string()
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
    context: &str,
    max: usize,
    roots: &[PathBuf],
) -> Result<Vec<crate::model::Recommendation>, String> {
    let graph = build(discover(roots));
    Ok(recommend(&graph, query, context, max))
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
    let mut event = events::detect_from_payload(env.runtime, ObservedVia::Hook, payload)?;
    // Normalize to the sanitized session id so all events of this run agree.
    event.session_id = env.session_id.clone();
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

    // Session continuity (issue #128): names this session's prompt mode
    // already surfaced become recommendation context.
    let session_ctx = env.session_recommended().join(" ");
    let start = Instant::now();
    let recs = match recommend_names(&skill_name, &session_ctx, 5, &roots) {
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

    // Same dedup key as the v0.3.x shell hook: prompt:<cksum(query)>.
    let dedup_key = format!("prompt:{}", cksum_decimal(query.as_bytes()));
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
    let recs = match recommend_names(&query, "", 5, &roots) {
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
    env.record_recommended(&names.iter().map(|n| n.to_string()).collect::<Vec<String>>());
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
    // Empty or unparseable stdin is not a hook payload: exit before
    // touching state dirs or the event log (matches the v0.3.x shell
    // hook, which exited early when jq could not parse the payload).
    let payload: Value = match serde_json::from_str::<Value>(input.trim()) {
        Ok(v) if !v.is_null() => v,
        _ => return Ok(()),
    };
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

#[cfg(test)]
mod tests {
    use super::cksum_decimal;

    // Expected values produced by `printf '%s' "$s" | cksum | cut -d' ' -f1`.
    #[test]
    fn cksum_matches_posix_cksum() {
        assert_eq!(cksum_decimal(b""), "4294967295");
        assert_eq!(cksum_decimal(b"a"), "1220704766");
        assert_eq!(cksum_decimal(b"skill:alpha-skill"), "1146759242");
        assert_eq!(cksum_decimal(b"prompt:abc"), "1161943104");
        assert_eq!(cksum_decimal(&vec![b'x'; 70000]), "4215398528");
    }
}
