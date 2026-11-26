//! Read-only observation of existing agent session data.
//!
//! Normalizes runtime-specific records into the shared Skill Usage Event
//! contract (`contracts/v1/skill-usage-event.schema.json`):
//!
//! - Claude Code transcript JSONL (`~/.claude/projects/**`)
//! - Codex rollout JSONL (`~/.codex/sessions/**/rollout-*.jsonl`)
//! - OpenCode `opencode.db` SQLite (`session_message` table)
//! - skill-graph event logs already in contract form (JSONL passthrough)
//!
//! Only SELECT-style reads are performed; no runtime data is modified.
//! Unreadable files/dirs, missing tables, and malformed lines are
//! tolerated and reported in `errors`/`lines_skipped` rather than
//! failing the scan. Valid-JSON lines matching no known shape are
//! ignored silently (format-drift tolerance).
//!
//! Co-observation hazard: transcript-derived events and the hook/plugin
//! event log describe the same deliveries. Pointing `observe` at a tree
//! containing both (e.g. `observe ~` when the event log lives under $HOME)
//! double-counts recommendations and halves conversion_rate — observe
//! transcripts OR the event log, not both, for metric use.

use crate::events::{
    self, Confidence, Detection, EventKind, ObservedVia, RecommendedSkill, Runtime,
    SkillUsageEvent, Trigger,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize)]
pub struct Observation {
    pub files_scanned: usize,
    pub lines_skipped: usize,
    pub sessions: usize,
    /// Load signals per skill name (excludes `mention`-level references).
    pub skill_loads: BTreeMap<String, usize>,
    pub loads: Vec<SkillLoad>,
    /// Normalized Skill Usage Events across all detected runtimes.
    pub events: Vec<SkillUsageEvent>,
    /// skill_load event count per runtime (`claude-code`, `codex`, `opencode`).
    pub runtimes: BTreeMap<String, usize>,
    /// Non-fatal per-file/per-table problems (missing tables, unreadable
    /// files, unsupported schema drift).
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillLoad {
    pub session_id: String,
    pub skill: String,
    pub timestamp: String,
    pub file: String,
    pub runtime: String,
    pub detection: String,
}

pub fn observe_path(path: &Path) -> Result<Observation, String> {
    let (files, collect_errors) = collect_observables(path);
    let mut files = files;
    files.sort();
    let mut obs = Observation::default();
    obs.errors.extend(collect_errors);
    let mut sessions = BTreeSet::new();
    for file in files {
        obs.files_scanned += 1;
        let loads_before = obs.loads.len();
        let result = if file.extension().and_then(|e| e.to_str()) == Some("db") {
            observe_db(&file, &mut obs, &mut sessions)
        } else {
            observe_jsonl(&file, &mut obs, &mut sessions)
        };
        if let Err(err) = result {
            obs.errors.push(format!("{}: {err}", file.display()));
        }
        for load in obs.loads.iter_mut().skip(loads_before) {
            load.file = file.display().to_string();
        }
    }
    obs.sessions = sessions.len();
    Ok(obs)
}

/// Best-effort session identifier on any parsed record, used only for the
/// `sessions` summary count (not for event attribution).
fn line_session_id(value: &Value) -> Option<String> {
    if let Some(sid) = value.get("sessionId").and_then(Value::as_str) {
        return Some(sid.to_string());
    }
    if let Some(sid) = value.get("session_id").and_then(Value::as_str) {
        return Some(sid.to_string());
    }
    let payload = value.get("payload")?;
    match value.get("type").and_then(Value::as_str) {
        Some("session_meta") => payload
            .get("session_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Some("token_usage_record") => payload
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Collect observable files plus non-fatal collection errors. A single
/// unreadable directory must not abort the whole scan, and files reachable
/// through multiple paths (symlinked/hardlinked transcripts) are scanned
/// once — canonicalizing the file dedups them so events are not
/// double-counted.
fn collect_observables(path: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut visited_dirs = std::collections::HashSet::new();
    let mut seen_files: std::collections::HashSet<ObsKey> = std::collections::HashSet::new();
    collect_observables_inner(
        path,
        &mut out,
        &mut errors,
        &mut visited_dirs,
        &mut seen_files,
    );
    (out, errors)
}

#[allow(clippy::too_many_arguments)]
fn collect_observables_inner(
    path: &Path,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
    visited_dirs: &mut std::collections::HashSet<PathBuf>,
    seen_files: &mut std::collections::HashSet<ObsKey>,
) {
    if path.is_file() {
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("jsonl") | Some("db")
        ) {
            push_observable(path, out, seen_files);
        }
        return;
    }
    if !path.is_dir() {
        errors.push(format!("not a file or directory: {}", path.display()));
        return;
    }
    // `is_dir` follows symlinks; a symlink cycle would recurse forever.
    // Canonicalize each directory and visit it at most once — this also
    // dedups bind mounts and overlapping roots.
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited_dirs.insert(canonical) {
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("{}: {e}", path.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let p = entry.path();
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            // Never recurse through symlinked directories (unbounded
            // scan risk), but a symlinked .jsonl/.db file is a single
            // bounded read and is followed — deduped by canonical target.
            if p.is_file()
                && matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("jsonl") | Some("db")
                )
            {
                push_observable(&p, out, seen_files);
            }
            continue;
        }
        if p.is_dir() {
            collect_observables_inner(&p, out, errors, visited_dirs, seen_files);
        } else if matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("jsonl") | Some("db")
        ) {
            push_observable(&p, out, seen_files);
        }
    }
}

/// File identity for observable dedup. On Unix, (dev, ino) unifies
/// hardlinks and symlink targets that canonicalize to different paths;
/// elsewhere (or when metadata fails) the canonical path is the key.
#[derive(Hash, PartialEq, Eq)]
enum ObsKey {
    File(u64, u64),
    Path(PathBuf),
}

fn observable_key(path: &Path) -> ObsKey {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // metadata() follows symlinks, so a symlinked .jsonl dedups
        // against its target just like canonicalize did.
        if let Ok(m) = fs::metadata(path) {
            return ObsKey::File(m.dev(), m.ino());
        }
    }
    ObsKey::Path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn push_observable(
    path: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<ObsKey>,
) {
    if seen.insert(observable_key(path)) {
        out.push(path.to_path_buf());
    }
}

/// Transcript/DB inputs are user data and can be huge; refuse to slurp
/// anything beyond this into memory.
const MAX_OBSERVE_FILE_BYTES: u64 = 256 * 1024 * 1024;

fn observe_jsonl(
    file: &Path,
    obs: &mut Observation,
    sessions: &mut BTreeSet<String>,
) -> Result<(), String> {
    if let Ok(meta) = fs::metadata(file)
        && meta.len() > MAX_OBSERVE_FILE_BYTES
    {
        return Err(format!("{}: file too large", file.display()));
    }
    let content = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut claude = ClaudeScan::default();
    let mut codex = CodexScan::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            obs.lines_skipped += 1;
            continue;
        };
        if let Some(sid) = line_session_id(&value) {
            sessions.insert(sid);
        }
        if try_passthrough_event(&value, obs) {
            continue;
        }
        if looks_like_codex_rollout(&value) {
            codex.feed(&value);
        } else if looks_like_claude_transcript(&value) {
            claude.feed(&value);
        }
        // Other JSON shapes are ignored silently (format drift tolerance).
    }
    codex.finish(obs);
    claude.finish(obs);
    Ok(())
}

/// Contract-form event logs are passed through after validation.
fn try_passthrough_event(value: &Value, obs: &mut Observation) -> bool {
    if value.get("kind").is_none() || value.get("schema_version").is_none() {
        return false;
    }
    let Ok(event) = serde_json::from_value::<SkillUsageEvent>(value.clone()) else {
        obs.lines_skipped += 1;
        return true;
    };
    if event.schema_version != events::SCHEMA_VERSION {
        obs.lines_skipped += 1;
        return true;
    }
    // Top-level additionalProperties:false — serde drops unknown keys
    // silently, so reject them explicitly to keep invalid input visible
    // in lines_skipped rather than normalized into valid output.
    const EVENT_KEYS: &[&str] = &[
        "schema_version",
        "kind",
        "runtime",
        "session_id",
        "timestamp",
        "observed_via",
        "prompt_hash",
        "trigger",
        "recommended",
        "emitted",
        "reason",
        "recommend_ms",
        "skill_name",
        "skill_path",
        "skill_digest",
        "detection",
        "confidence",
        "tool",
    ];
    if value
        .as_object()
        .is_some_and(|o| o.keys().any(|k| !EVENT_KEYS.contains(&k.as_str())))
    {
        obs.lines_skipped += 1;
        return true;
    }
    // Enforce the schema's kind-conditional required fields; the Rust
    // struct cannot distinguish "missing" from "empty" for Vec fields,
    // and an explicit JSON null must count as absent (serde maps it to
    // None, which would re-serialize without the key).
    let required: &[&str] = match event.kind {
        EventKind::PromptSubmit => &["prompt_hash"],
        EventKind::Recommendation => &["recommended", "emitted", "reason", "trigger"],
        EventKind::SkillLoad => &["skill_name", "detection", "confidence"],
    };
    if required
        .iter()
        .any(|k| !value.get(*k).is_some_and(|v| !v.is_null()))
    {
        obs.lines_skipped += 1;
        return true;
    }
    // `recommended` item constraints apply whenever the field is present,
    // not only on recommendation events (schema property-level rule).
    // Names must be non-empty valid skill names; items may carry only
    // name/digest (schema additionalProperties:false).
    if let Some(items) = value.get("recommended").and_then(Value::as_array) {
        let bad_item = items.iter().any(|item| {
            let name_ok = item
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| !n.is_empty() && crate::parser::valid_skill_name(n));
            let keys_ok = item
                .as_object()
                .is_some_and(|o| o.keys().all(|k| k == "name" || k == "digest"));
            !name_ok || !keys_ok
        });
        if bad_item {
            obs.lines_skipped += 1;
            return true;
        }
    }
    record_event(event, obs);
    true
}

fn looks_like_codex_rollout(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            "session_meta"
                | "event_msg"
                | "response_item"
                | "turn_context"
                | "world_state"
                | "token_usage_record"
        )
    )
}

fn looks_like_claude_transcript(value: &Value) -> bool {
    value.get("sessionId").is_some()
        && value
            .get("message")
            .and_then(|m| m.get("content"))
            .is_some()
}

fn record_event(event: SkillUsageEvent, obs: &mut Observation) {
    // The contract requires skill_name on skill_load; unattributable
    // observations are dropped rather than emitted schema-invalid. An
    // empty string is equally unattributable.
    if event.kind == EventKind::SkillLoad
        && event
            .skill_name
            .as_deref()
            .map(str::is_empty)
            .unwrap_or(true)
    {
        return;
    }
    if event.counts_as_load() {
        *obs.runtimes
            .entry(event.runtime.as_str().to_string())
            .or_default() += 1;
        if let Some(name) = &event.skill_name {
            *obs.skill_loads.entry(name.clone()).or_default() += 1;
            obs.loads.push(SkillLoad {
                session_id: event.session_id.clone(),
                skill: name.clone(),
                timestamp: event.timestamp.clone(),
                file: String::new(),
                runtime: event.runtime.as_str().to_string(),
                detection: event
                    .detection
                    .map(detection_name)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    obs.events.push(event);
}

fn detection_name(d: Detection) -> &'static str {
    match d {
        Detection::NativeInvocation => "native_invocation",
        Detection::FileRead => "file_read",
        Detection::PathReference => "path_reference",
        Detection::Inferred => "inferred",
        Detection::Mention => "mention",
        Detection::Applied => "applied",
    }
}

// ---------------------------------------------------------------------------
// Claude Code transcript
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ClaudeScan {
    /// (tool_use_id, session_id, timestamp, tool name, input)
    uses: Vec<(String, String, String, String, Value)>,
    /// tool_use_id -> output text
    results: HashMap<String, String>,
    /// (session_id, timestamp, text) user prompts
    prompts: Vec<(String, String, String)>,
}

impl ClaudeScan {
    fn feed(&mut self, value: &Value) {
        let Some(session_id) = value.get("sessionId").and_then(Value::as_str) else {
            return;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let content = value.get("message").and_then(|m| m.get("content"));
        let is_user_line = value.get("type").and_then(Value::as_str) == Some("user");
        match content {
            Some(Value::Array(items)) => {
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            let id = item
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let input = item.get("input").cloned().unwrap_or(Value::Null);
                            self.uses.push((
                                id,
                                session_id.to_string(),
                                timestamp.clone(),
                                name,
                                input,
                            ));
                        }
                        Some("tool_result") => {
                            let id = item
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let text =
                                events::response_text(item.get("content").unwrap_or(&Value::Null));
                            self.results.insert(id, text);
                        }
                        Some("text") if is_user_line => {
                            if let Some(text) = item.get("text").and_then(Value::as_str) {
                                self.prompts.push((
                                    session_id.to_string(),
                                    timestamp.clone(),
                                    text.to_string(),
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(Value::String(text)) if is_user_line => {
                self.prompts
                    .push((session_id.to_string(), timestamp.clone(), text.clone()));
            }
            _ => {}
        }
    }

    fn finish(self, obs: &mut Observation) {
        for (session, ts, text) in &self.prompts {
            if is_meaningful_prompt(text) {
                record_event(
                    SkillUsageEvent::prompt_submit(
                        Runtime::ClaudeCode,
                        ObservedVia::Transcript,
                        session,
                        ts,
                        text,
                    ),
                    obs,
                );
            }
        }
        for (id, session, ts, tool, input) in &self.uses {
            let output = self.results.get(id).map(String::as_str).unwrap_or("");
            for c in events::classify_tool_access_all(tool, input, output) {
                record_event(
                    SkillUsageEvent::skill_load(
                        Runtime::ClaudeCode,
                        ObservedVia::Transcript,
                        session,
                        ts,
                        Some(tool.clone()),
                        c.skill_name,
                        c.skill_path,
                        c.detection,
                        c.confidence,
                    ),
                    obs,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Codex rollout JSONL
// ---------------------------------------------------------------------------

struct PendingCall {
    call_id: String,
    session_id: String,
    timestamp: String,
    /// (skill path, access kind) pairs extracted from exec JS commands
    accesses: Vec<(String, events::Access)>,
}

#[derive(Default)]
struct CodexScan {
    session_id: String,
    pending: Vec<PendingCall>,
    outputs: HashMap<String, String>,
    /// (session, timestamp, text)
    prompts: Vec<(String, String, String)>,
    injected: Vec<SkillUsageEvent>,
}

impl CodexScan {
    fn feed(&mut self, value: &Value) {
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(payload) = value.get("payload") else {
            return;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(id) = payload
                    .get("session_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                {
                    self.session_id = id.to_string();
                }
            }
            Some("token_usage_record") => {
                if self.session_id.is_empty()
                    && let Some(id) = payload.get("session_id").and_then(Value::as_str)
                {
                    self.session_id = id.to_string();
                }
            }
            Some("response_item") => match payload.get("type").and_then(Value::as_str) {
                Some("custom_tool_call") => {
                    let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
                    if name != "exec" {
                        return;
                    }
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let input = payload.get("input").and_then(Value::as_str).unwrap_or("");
                    let mut accesses: Vec<(String, events::Access)> = Vec::new();
                    for cmd in events::extract_exec_commands(input) {
                        accesses.extend(events::classify_command(&cmd));
                    }
                    // Fallback: unparsed JS may still reference SKILL.md in
                    // the raw input text.
                    if accesses.is_empty() && input.contains("SKILL.md") {
                        accesses.extend(events::classify_command(input));
                    }
                    // Dedup by path keeping the stronger access so one
                    // call touching the same SKILL.md twice counts once.
                    // HashMap index keeps this O(1) per access — a hostile
                    // transcript could otherwise force quadratic scans.
                    let mut deduped: Vec<(String, events::Access)> = Vec::new();
                    let mut dedup_index: std::collections::HashMap<String, usize> =
                        std::collections::HashMap::new();
                    for (path, access) in accesses {
                        match dedup_index.get(&path) {
                            Some(&i)
                                if access == events::Access::Read
                                    && deduped[i].1 == events::Access::Reference =>
                            {
                                deduped[i].1 = events::Access::Read;
                            }
                            Some(_) => {}
                            None => {
                                dedup_index.insert(path.clone(), deduped.len());
                                deduped.push((path, access));
                            }
                        }
                    }
                    if !deduped.is_empty() {
                        self.pending.push(PendingCall {
                            call_id,
                            session_id: self.session_id.clone(),
                            timestamp: timestamp.clone(),
                            accesses: deduped,
                        });
                    }
                }
                Some("custom_tool_call_output") => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let text = codex_output_text(payload);
                    if !call_id.is_empty() && !text.is_empty() {
                        self.outputs.insert(call_id, text);
                    }
                }
                Some("message") => {
                    let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                    let text = payload
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|c| c.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    match role {
                        "user" if is_meaningful_prompt(&text) => {
                            self.prompts
                                .push((self.session_id.clone(), timestamp.clone(), text));
                        }
                        "developer" if text.starts_with("skill-graph:") => {
                            if let Some(event) =
                                parse_injected_recommendation(&self.session_id, &timestamp, &text)
                            {
                                self.injected.push(event);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn finish(self, obs: &mut Observation) {
        for event in self.injected {
            record_event(event, obs);
        }
        for (session, ts, text) in &self.prompts {
            record_event(
                SkillUsageEvent::prompt_submit(
                    Runtime::Codex,
                    ObservedVia::Transcript,
                    session,
                    ts,
                    text,
                ),
                obs,
            );
        }
        for pending in &self.pending {
            let output = self
                .outputs
                .get(&pending.call_id)
                .map(String::as_str)
                .unwrap_or("");
            for (path, access) in &pending.accesses {
                let c = match access {
                    events::Access::Read => classify_with_output(path, output),
                    events::Access::Reference => events::LoadClassification {
                        skill_name: events::skill_name_from_path(path),
                        skill_path: Some(path.clone()),
                        detection: Detection::Mention,
                        confidence: Confidence::Low,
                    },
                };
                record_event(
                    SkillUsageEvent::skill_load(
                        Runtime::Codex,
                        ObservedVia::Transcript,
                        &pending.session_id,
                        &pending.timestamp,
                        Some("exec".to_string()),
                        c.skill_name,
                        c.skill_path,
                        c.detection,
                        c.confidence,
                    ),
                    obs,
                );
            }
        }
    }
}

fn classify_with_output(path: &str, output: &str) -> events::LoadClassification {
    // Same rule as the hook path: frontmatter in the captured output
    // upgrades a read to a verified file_read.
    if let Some(name) = events::frontmatter_name(output) {
        events::LoadClassification {
            skill_name: Some(name),
            skill_path: Some(path.to_string()),
            detection: Detection::FileRead,
            confidence: Confidence::High,
        }
    } else {
        events::LoadClassification {
            skill_name: events::skill_name_from_path(path),
            skill_path: Some(path.to_string()),
            detection: Detection::PathReference,
            confidence: Confidence::Medium,
        }
    }
}

/// Extract the command output text from a `custom_tool_call_output`
/// payload: concatenated `output[].text` parts, unwrapping the inner
/// `{"output": "..."}` JSON envelope Codex exec writes.
fn codex_output_text(payload: &Value) -> String {
    let raw = events::response_text(payload.get("output").unwrap_or(&Value::Null));
    // The text ends with a JSON object {"chunk_id":...,"output":"..."}.
    if let Some(start) = raw.find('{') {
        let tail = &raw[start..];
        let parsed = serde_json::from_str::<Value>(tail).or_else(|_| {
            serde_json::Deserializer::from_str(tail)
                .into_iter::<Value>()
                .next()
                .ok_or(())
                .and_then(|r| r.map_err(|_| ()))
        });
        if let Ok(v) = parsed
            && let Some(inner) = v.get("output").and_then(Value::as_str)
        {
            return inner.to_string();
        }
    }
    raw
}

/// Parse an injected `skill-graph:` system message (the recommendation the
/// hook/plugin pushed into context) back into a `recommendation` event.
/// This is the transcript-side evidence that a recommendation was delivered.
fn parse_injected_recommendation(
    session_id: &str,
    timestamp: &str,
    text: &str,
) -> Option<SkillUsageEvent> {
    let body = text.strip_prefix("skill-graph: ")?;
    if !body.contains(" — ") {
        return None;
    }
    let trigger = if body.starts_with('\'') {
        Trigger::SkillLoad
    } else {
        Trigger::PromptSubmit
    };
    // Names sit between the last " — " and the first "." after it.
    let after_dash = body.rsplit(" — ").next().unwrap_or(body);
    let names_str = after_dash.split('.').next().unwrap_or("");
    let names: Vec<RecommendedSkill> = names_str
        .split(',')
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && crate::parser::valid_skill_name(n))
        .map(|n| RecommendedSkill {
            name: n.to_string(),
            digest: None,
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    Some(SkillUsageEvent::recommendation(
        Runtime::Codex,
        ObservedVia::Transcript,
        session_id,
        timestamp,
        None,
        trigger,
        names,
        true,
        "injected",
        None,
    ))
}

// ---------------------------------------------------------------------------
// OpenCode opencode.db (SQLite, read-only)
// ---------------------------------------------------------------------------

fn observe_db(
    file: &Path,
    obs: &mut Observation,
    sessions: &mut BTreeSet<String>,
) -> Result<(), String> {
    let conn = open_sqlite_readonly(file)?;
    // Schema drift tolerance: the table may not exist in older/newer builds.
    let has_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_message'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_table == 0 {
        return Err("session_message table not found; skipped".to_string());
    }
    // `seq` exists in current opencode.db builds but is not contractual;
    // fall back to rowid so schema drift doesn't skip the whole DB.
    let has_seq: bool = conn
        .prepare("PRAGMA table_info(session_message)")
        .and_then(|mut s| {
            Ok(s.query_map([], |r| r.get::<_, String>(1))?
                .any(|c| c.map(|n| n == "seq").unwrap_or(false)))
        })
        .unwrap_or(false);
    let order_col = if has_seq { "seq" } else { "rowid" };
    let sql = format!(
        "SELECT session_id, type, data, time_created FROM session_message ORDER BY session_id, {order_col}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| format!("query: {e}"))?;
    for row in rows {
        // A single malformed row (NULL column, type drift) must not abort
        // the whole database scan.
        let Ok((session_id, kind, data, created_ms)) = row else {
            obs.lines_skipped += 1;
            continue;
        };
        sessions.insert(session_id.clone());
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            obs.lines_skipped += 1;
            continue;
        };
        let ts = events::epoch_ms_to_rfc3339(created_ms);
        match kind.as_str() {
            "user" => {
                if let Some(text) = value.get("text").and_then(Value::as_str)
                    && is_meaningful_prompt(text)
                {
                    record_event(
                        SkillUsageEvent::prompt_submit(
                            Runtime::Opencode,
                            ObservedVia::Db,
                            &session_id,
                            &ts,
                            text,
                        ),
                        obs,
                    );
                }
            }
            "assistant" => {
                let content = value
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in content {
                    if item.get("type").and_then(Value::as_str) != Some("tool") {
                        continue;
                    }
                    let tool = item.get("name").and_then(Value::as_str).unwrap_or("");
                    let input = item
                        .get("state")
                        .and_then(|s| s.get("input"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let output = events::response_text(
                        item.get("state")
                            .and_then(|s| s.get("content"))
                            .unwrap_or(&Value::Null),
                    );
                    for c in events::classify_tool_access_all(tool, &input, &output) {
                        record_event(
                            SkillUsageEvent::skill_load(
                                Runtime::Opencode,
                                ObservedVia::Db,
                                &session_id,
                                &ts,
                                Some(tool.to_string()),
                                c.skill_name,
                                c.skill_path,
                                c.detection,
                                c.confidence,
                            ),
                            obs,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn open_sqlite_readonly(path: &Path) -> Result<rusqlite::Connection, String> {
    use rusqlite::OpenFlags;
    match rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => Ok(conn),
        Err(first) => {
            // WAL-mode databases may refuse a pure read-only open when the
            // -shm/-wal sidecars are absent. Retry through a `mode=ro` URI:
            // it still forbids writes to the database file itself (no WAL
            // recovery or journal writes), unlike a plain read-write open.
            // SQLite may still create -shm/-wal sidecars, which is inherent
            // to reading a live WAL database and modifies no stored rows.
            // Percent-encode the path portion: spaces, `?`, `#`, `%` are
            // legal in filesystem paths but meaningful in a URI.
            let encoded: String = path
                .to_string_lossy()
                .chars()
                .flat_map(|c| match c {
                    '%' => "%25".chars().collect::<Vec<_>>(),
                    '?' => "%3F".chars().collect(),
                    '#' => "%23".chars().collect(),
                    ' ' => "%20".chars().collect(),
                    c => vec![c],
                })
                .collect();
            let uri = format!("file:{encoded}?mode=ro");
            rusqlite::Connection::open_with_flags(
                &uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|e| format!("open db: {first}; retry: {e}"))
        }
    }
}

/// Prompt text worth recording: non-empty and not an environment/context
/// envelope injected by the runtime itself.
fn is_meaningful_prompt(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && !t.starts_with("<environment_context")
        && !t.starts_with("<skills_instructions>")
}

/// Funnel metrics defined by docs/specs/v0.3.0-observation-pilot.md,
/// computed from normalized events. Read-only; printed by
/// `observe --summary`.
pub fn summary(obs: &Observation) -> Value {
    let all: Vec<&crate::events::SkillUsageEvent> = obs.events.iter().collect();
    let mut out = funnel_for(&all).into_json();
    let mut by_runtime = serde_json::Map::new();
    for runtime in ["claude-code", "codex", "opencode"] {
        let subset: Vec<&crate::events::SkillUsageEvent> = all
            .iter()
            .copied()
            .filter(|e| e.runtime.as_str() == runtime)
            .collect();
        if !subset.is_empty() {
            by_runtime.insert(runtime.to_string(), funnel_for(&subset).into_json());
        }
    }
    out["by_runtime"] = Value::Object(by_runtime);
    // Spec §2: funnel metrics aggregate per runtime AND per session.
    // Sessions with an empty id are excluded (unattributable); the map is
    // capped so a huge log cannot produce unbounded output.
    const MAX_SUMMARY_SESSIONS: usize = 500;
    let mut by_session_map = serde_json::Map::new();
    let mut session_events: std::collections::BTreeMap<
        String,
        Vec<&crate::events::SkillUsageEvent>,
    > = std::collections::BTreeMap::new();
    for e in &all {
        if !e.session_id.is_empty() {
            session_events
                .entry(format!("{}:{}", e.runtime.as_str(), e.session_id))
                .or_default()
                .push(*e);
        }
    }
    let total_sessions = session_events.len();
    for (session_id, events) in session_events.into_iter().take(MAX_SUMMARY_SESSIONS) {
        by_session_map.insert(session_id, funnel_for(&events).into_json());
    }
    out["by_session"] = Value::Object(by_session_map);
    if total_sessions > MAX_SUMMARY_SESSIONS {
        out["by_session_omitted"] = json!(total_sessions - MAX_SUMMARY_SESSIONS);
    }
    out["events"] = json!(obs.events.len());
    out["files_scanned"] = json!(obs.files_scanned);
    out["lines_skipped"] = json!(obs.lines_skipped);
    // Collection errors (unreadable dirs/files, oversized inputs) mean the
    // aggregates above are partial — surface them like by_session does.
    const MAX_SUMMARY_ERRORS: usize = 100;
    out["collection_errors"] = json!(obs.errors.len());
    if !obs.errors.is_empty() {
        out["errors"] = json!(
            obs.errors
                .iter()
                .take(MAX_SUMMARY_ERRORS)
                .collect::<Vec<_>>()
        );
        if obs.errors.len() > MAX_SUMMARY_ERRORS {
            out["errors_omitted"] = json!(obs.errors.len() - MAX_SUMMARY_ERRORS);
        }
    }
    out
}

/// RFC 3339 (`YYYY-MM-DDTHH:MM:SS[.fff]Z` or `±hh:mm`) → epoch seconds.
/// None for unparseable input; callers treat that as "unknown time".
fn rfc3339_secs(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |i: usize, n: usize| -> Option<i64> { s.get(i..i + n)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if b.get(10) != Some(&b'T') || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // days-from-civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
    }
    match b.get(i) {
        Some(b'Z') | None => Some(secs),
        Some(sign @ (b'+' | b'-')) => {
            let off_h = num(i + 1, 2)?;
            let off_m = if b.get(i + 3) == Some(&b':') {
                num(i + 4, 2)?
            } else {
                num(i + 3, 2)?
            };
            let off = off_h * 3600 + off_m * 60;
            Some(secs + if *sign == b'+' { -off } else { off })
        }
        _ => None,
    }
}

#[derive(Default)]
struct Funnel {
    sessions: BTreeSet<String>,
    prompt_submits: usize,
    recommendations: usize,
    emitted: usize,
    load_grade_loads: usize,
    sessions_with_loads: BTreeSet<String>,
    converted: usize,
    unnecessary_loads: usize,
    deltas_ms: Vec<i64>,
    emitted_per_session: BTreeMap<String, usize>,
}

/// Compute funnel metrics over a set of events (overall or one runtime).
fn funnel_for(events: &[&crate::events::SkillUsageEvent]) -> Funnel {
    use crate::events::EventKind;
    let mut f = Funnel::default();
    // Stable per-session ordering: (timestamp, input order).
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by_key(|&i| (rfc3339_secs(&events[i].timestamp).unwrap_or(i64::MAX), i));

    // Top-level counters cover every event, including unattributable
    // ones (empty session_id) — they just never join a session funnel.
    for &i in &order {
        let e = events[i];
        match e.kind {
            EventKind::PromptSubmit => f.prompt_submits += 1,
            EventKind::Recommendation => {
                f.recommendations += 1;
                if e.emitted == Some(true) {
                    f.emitted += 1;
                    if !e.session_id.is_empty() {
                        *f.emitted_per_session
                            .entry(format!("{}:{}", e.runtime.as_str(), e.session_id))
                            .or_default() += 1;
                    }
                }
            }
            EventKind::SkillLoad if e.counts_as_load() => {
                f.load_grade_loads += 1;
                if !e.session_id.is_empty() {
                    f.sessions_with_loads.insert(format!(
                        "{}:{}",
                        e.runtime.as_str(),
                        e.session_id
                    ));
                }
            }
            _ => {}
        }
    }

    // Session-scoped joins group by (runtime, session_id): ids are only
    // unique within a runtime, and events with no session id must not
    // merge into one pseudo-session across unrelated files.
    let mut by_session: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for &i in &order {
        let e = events[i];
        if e.session_id.is_empty() {
            continue;
        }
        f.sessions
            .insert(format!("{}:{}", e.runtime.as_str(), e.session_id));
        by_session
            .entry((e.runtime.as_str(), e.session_id.as_str()))
            .or_default()
            .push(i);
    }
    for idxs in by_session.values() {
        session_funnel(&mut f, events, idxs);
    }
    f
}

/// conversion / unnecessary-load / timeliness for one session's
/// timestamp-ordered event indexes.
fn session_funnel(f: &mut Funnel, events: &[&crate::events::SkillUsageEvent], idxs: &[usize]) {
    use crate::events::EventKind;
    let recs: Vec<usize> = idxs
        .iter()
        .copied()
        .filter(|&i| events[i].kind == EventKind::Recommendation)
        .collect();
    let loads: Vec<usize> = idxs
        .iter()
        .copied()
        .filter(|&i| events[i].counts_as_load())
        .collect();
    // Positions within the timestamp-sorted session order; the no-
    // timestamp fallback must compare order positions, not raw indices.
    let pos: std::collections::HashMap<usize, usize> =
        idxs.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    // recommended names from emitted recommendations only — loads of
    // names we never surfaced count as unnecessary per the pilot def.
    let mut recommended_names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for &ri in &recs {
        let e = events[ri];
        if e.emitted == Some(true) {
            for r in &e.recommended {
                recommended_names.insert(r.name.as_str());
            }
        }
    }
    for &li in &loads {
        let name = events[li].skill_name.as_deref().unwrap_or("");
        if !recommended_names.contains(name) {
            f.unnecessary_loads += 1;
        }
    }

    // Greedy conversion: each emitted recommendation converts the first
    // later load-grade load naming one of its recommendations; each load
    // converts at most one recommendation.
    let mut consumed: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for &ri in &recs {
        let rec = events[ri];
        if rec.emitted != Some(true) {
            continue;
        }
        let names: std::collections::BTreeSet<&str> =
            rec.recommended.iter().map(|r| r.name.as_str()).collect();
        if names.is_empty() {
            continue;
        }
        let rec_ts = rfc3339_secs(&rec.timestamp);
        for &li in &loads {
            if consumed.contains(&li) {
                continue;
            }
            let load = events[li];
            if !names.contains(load.skill_name.as_deref().unwrap_or("")) {
                continue;
            }
            let load_ts = rfc3339_secs(&load.timestamp);
            let after = match (rec_ts, load_ts) {
                (Some(a), Some(b)) => b >= a,
                _ => pos.get(&li).zip(pos.get(&ri)).is_some_and(|(l, r)| l > r),
            };
            if !after {
                continue;
            }
            consumed.insert(li);
            f.converted += 1;
            if let (Some(a), Some(b)) = (rec_ts, load_ts) {
                f.deltas_ms.push((b - a) * 1000);
            }
            break;
        }
    }
}

impl Funnel {
    fn into_json(self) -> Value {
        let pct = |num: usize, den: usize| -> Value {
            if den == 0 {
                Value::Null
            } else {
                json!((num as f64 / den as f64 * 1000.0).round() / 10.0)
            }
        };
        let mut deltas = self.deltas_ms;
        deltas.sort_unstable();
        let percentile = |p: f64| -> Value {
            if deltas.is_empty() {
                return Value::Null;
            }
            let idx = ((deltas.len() - 1) as f64 * p).round() as usize;
            json!(deltas[idx])
        };
        let eps: Vec<usize> = self.emitted_per_session.values().copied().collect();
        let (avg, max) = if eps.is_empty() {
            (Value::Null, Value::Null)
        } else {
            (
                json!(
                    (eps.iter().sum::<usize>() as f64 / eps.len() as f64 * 100.0).round() / 100.0
                ),
                json!(eps.iter().max().unwrap()),
            )
        };
        json!({
            "sessions": self.sessions.len(),
            "prompt_submits": self.prompt_submits,
            "recommendations": self.recommendations,
            "emitted_recommendations": self.emitted,
            "delivery_rate_pct": pct(self.emitted, self.recommendations),
            "load_grade_loads": self.load_grade_loads,
            "sessions_with_loads": self.sessions_with_loads.len(),
            "loads_per_session": if self.sessions.is_empty() { Value::Null } else {
                json!((self.load_grade_loads as f64 / self.sessions.len() as f64 * 100.0).round() / 100.0)
            },
            "converted_recommendations": self.converted,
            "conversion_rate_pct": pct(self.converted, self.emitted),
            "unnecessary_loads": self.unnecessary_loads,
            "unnecessary_load_rate_pct": pct(self.unnecessary_loads, self.load_grade_loads),
            "timeliness_ms": {
                "count": deltas.len(),
                "p50": percentile(0.50),
                "p95": percentile(0.95),
                "max": deltas.last().map(|d| json!(d)).unwrap_or(Value::Null),
            },
            "context_consumption": {
                "emitted_total": self.emitted,
                "emitted_per_session_avg": avg,
                "emitted_per_session_max": max,
            },
        })
    }
}
