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
    /// Set once the retained-events cap notices truncation; further
    /// rejections stay silent so `errors` cannot grow with input size.
    #[serde(skip)]
    pub events_truncated: bool,
    #[serde(skip)]
    pub loads_truncated: bool,
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
        // Size cap applies to every observable format — .db inputs are
        // scanned row-by-row and would otherwise have no bound. For a
        // SQLite database the un-checkpointed WAL is read too, so its
        // size counts against the same cap.
        let mut size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        if file.extension().and_then(|e| e.to_str()) == Some("db") {
            let mut wal = file.as_os_str().to_os_string();
            wal.push("-wal");
            if let Ok(meta) = fs::metadata(Path::new(&wal)) {
                size = size.saturating_add(meta.len());
            }
        }
        if size > MAX_OBSERVE_FILE_BYTES {
            obs.errors
                .push(format!("{}: file too large", file.display()));
            continue;
        }
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
/// Aggregate bound on observable inputs: per-file caps alone still allow
/// `observe ~` to read an unbounded number of files into memory.
const MAX_OBSERVE_FILES: usize = 4096;

fn collect_observables(path: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut visited_dirs = std::collections::HashSet::new();
    let mut seen_files: std::collections::HashSet<ObsKey> = std::collections::HashSet::new();
    let mut cap_noted = false;
    collect_observables_inner(
        path,
        &mut out,
        &mut errors,
        &mut visited_dirs,
        &mut seen_files,
        &mut cap_noted,
    );
    (out, errors)
}

/// Record the aggregate-cap truncation once, however many directories
/// hit the limit.
fn note_observable_cap(out_len: usize, errors: &mut Vec<String>, noted: &mut bool) {
    if out_len >= MAX_OBSERVE_FILES && !*noted {
        *noted = true;
        errors.push(format!(
            "observable inputs truncated at {MAX_OBSERVE_FILES} files"
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_observables_inner(
    path: &Path,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
    visited_dirs: &mut std::collections::HashSet<PathBuf>,
    seen_files: &mut std::collections::HashSet<ObsKey>,
    cap_noted: &mut bool,
) {
    if out.len() >= MAX_OBSERVE_FILES {
        note_observable_cap(out.len(), errors, cap_noted);
        return;
    }
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
        if out.len() >= MAX_OBSERVE_FILES {
            note_observable_cap(out.len(), errors, cap_noted);
            break;
        }
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
            collect_observables_inner(&p, out, errors, visited_dirs, seen_files, cap_noted);
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
    let Ok(mut event) = serde_json::from_value::<SkillUsageEvent>(value.clone()) else {
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
    // skill_load requires an attributable name; an empty string passes
    // the presence check above but record_event would drop it silently,
    // so count it here instead.
    if event.kind == EventKind::SkillLoad
        && event
            .skill_name
            .as_deref()
            .map(str::is_empty)
            .unwrap_or(true)
    {
        obs.lines_skipped += 1;
        return true;
    }
    // Normalize payload-controlled fields to the bounds the event
    // constructors apply; a contract-shaped log line can otherwise carry
    // unbounded strings into downstream map keys.
    event.session_id = events::sanitize_session_id(&event.session_id);
    event.skill_path = event.skill_path.map(|p| p.chars().take(1024).collect());
    event.tool = event.tool.map(|t| t.chars().take(256).collect());
    event.skill_name = event.skill_name.map(|n| n.chars().take(256).collect());
    // Remaining string fields get the same treatment: a single hostile
    // log line must not inject file-cap-sized strings into retained
    // events. Bounds match or exceed what the constructors produce.
    event.reason = event.reason.map(|r| r.chars().take(1024).collect());
    event.prompt_hash = event.prompt_hash.map(|h| h.chars().take(128).collect());
    event.skill_digest = event.skill_digest.map(|d| d.chars().take(128).collect());
    event.timestamp = event.timestamp.chars().take(64).collect();
    event.recommended.truncate(events::MAX_RECOMMENDED);
    for item in &mut event.recommended {
        item.name = item.name.chars().take(256).collect();
        if let Some(d) = &mut item.digest {
            *d = d.chars().take(128).collect();
        }
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

/// Aggregate bound on retained events and per-session load records.
/// Per-file caps alone allow a single max-size input to accumulate
/// millions of events (~3M lines in a 256 MiB JSONL) — each retaining
/// several heap strings — so the vecs themselves are bounded. Counter
/// maps (`runtimes`, `skill_loads`) stay accurate; only the retained
/// per-event detail is truncated, with a single notice in `errors`.
const MAX_OBSERVE_EVENTS: usize = 1_000_000;
const MAX_OBSERVE_LOADS: usize = 1_000_000;

/// Returns true while `len` is below `max`; emits one truncation notice
/// per collection via `noted`, which latches so post-cap rejections do
/// not grow `errors` with input size.
fn under_cap(
    len: usize,
    max: usize,
    what: &str,
    noted: &mut bool,
    errors: &mut Vec<String>,
) -> bool {
    if len < max {
        return true;
    }
    if !*noted {
        *noted = true;
        errors.push(format!("{what} truncated at {max}"));
    }
    false
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
            if under_cap(
                obs.loads.len(),
                MAX_OBSERVE_LOADS,
                "loads",
                &mut obs.loads_truncated,
                &mut obs.errors,
            ) {
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
    }
    if under_cap(
        obs.events.len(),
        MAX_OBSERVE_EVENTS,
        "events",
        &mut obs.events_truncated,
        &mut obs.errors,
    ) {
        obs.events.push(event);
    }
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
            // One captured output can cover several SKILL.md reads; its
            // frontmatter block belongs to the first file emitted, so only
            // the first Read access may claim it.
            let mut frontmatter_available = true;
            for (path, access) in &pending.accesses {
                let c = match access {
                    events::Access::Read => {
                        let verify = frontmatter_available;
                        frontmatter_available = false;
                        if verify {
                            classify_with_output(path, output)
                        } else {
                            events::LoadClassification {
                                skill_name: events::skill_name_from_path(path),
                                skill_path: Some(path.clone()),
                                detection: events::Detection::PathReference,
                                confidence: events::Confidence::Medium,
                            }
                        }
                    }
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

/// RFC 3339 (`YYYY-MM-DDTHH:MM:SS[.fff]Z` or `±hh:mm`) → epoch
/// milliseconds. Strict calendar validation: real month lengths with
/// leap-year-aware February, hour 0–23, minute 0–59, second 0–60 (leap
/// second), fractional seconds truncated to millisecond precision, and
/// a bounded `±hh:mm` offset. None for unparseable or out-of-range
/// input; callers treat that as "unknown time" so an invalid timestamp
/// can never distort `timeliness_ms` (ordering/conversion fall back to
/// input order, timeliness deltas require both endpoints).
fn rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    // Fixed-width digit fields only — a sign or other non-digit in a
    // numeric position is not RFC 3339.
    let num = |i: usize, n: usize| -> Option<i64> {
        let field = s.get(i..i + n)?;
        if !field.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        field.parse().ok()
    };
    if b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Month length with the Gregorian leap-year rule (divisible by 4,
    // except centuries unless divisible by 400).
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=dim).contains(&d) {
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
    // Fractional seconds: keep millisecond precision so sub-second
    // timeliness deltas are not quantized away.
    let mut millis = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        // RFC 3339 requires at least one digit after the decimal point.
        if !b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut digits = 0;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            if digits < 3 {
                millis = millis * 10 + i64::from(b[i] - b'0');
            }
            digits += 1;
            i += 1;
        }
        while digits < 3 {
            millis *= 10;
            digits += 1;
        }
    }
    let epoch_ms = secs * 1000 + millis;
    match b.get(i) {
        // 'Z' or absent offset must terminate the timestamp.
        Some(b'Z') if i + 1 == b.len() => Some(epoch_ms),
        None => Some(epoch_ms),
        Some(sign @ (b'+' | b'-')) => {
            let (off_h, off_m, end) = if b.get(i + 3) == Some(&b':') {
                (num(i + 1, 2)?, num(i + 4, 2)?, i + 6)
            } else {
                (num(i + 1, 2)?, num(i + 3, 2)?, i + 5)
            };
            if end != b.len() || off_h > 23 || off_m > 59 {
                return None;
            }
            let off = (off_h * 3600 + off_m * 60) * 1000;
            Some(epoch_ms + if *sign == b'+' { -off } else { off })
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
    /// Load-grade loads carrying a non-empty session id — the only ones
    /// that can join a session funnel. Session-scoped rates use this as
    /// their denominator so unattributable loads cannot silently deflate
    /// them; the unattributable share stays visible via the difference
    /// with `load_grade_loads`.
    session_attributed_loads: usize,
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
    order.sort_by_cached_key(|&i| (rfc3339_ms(&events[i].timestamp).unwrap_or(i64::MAX), i));

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
                    f.session_attributed_loads += 1;
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

    // Index loads by skill name and parse timestamps once: the
    // conversion scan below must not be O(recommendations × loads) —
    // a large contract-shaped event log would otherwise hang
    // `observe --summary` (the by_session cap does not bound the
    // top-level funnel, which runs over all events).
    let mut loads_by_name: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    for &li in &loads {
        loads_by_name
            .entry(events[li].skill_name.as_deref().unwrap_or(""))
            .or_default()
            .push(li);
    }
    let timestamps: std::collections::HashMap<usize, Option<i64>> = idxs
        .iter()
        .map(|&i| (i, rfc3339_ms(&events[i].timestamp)))
        .collect();

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
        let rec_ts = timestamps[&ri];
        // Union of candidate loads across recommended names, visited in
        // session order — equivalent to scanning `loads` while skipping
        // non-matching names, but only touches relevant loads.
        let mut candidates: Vec<usize> = names
            .iter()
            .filter_map(|name| loads_by_name.get(*name))
            .flatten()
            .copied()
            .collect();
        candidates.sort_by_key(|&li| pos[&li]);
        for li in candidates {
            if consumed.contains(&li) {
                continue;
            }
            let load_ts = timestamps[&li];
            // When either timestamp is unparseable, fall back to input
            // (encounter) order — events are collected in transcript/log
            // order, which is the best available chronology. The
            // timestamp-sorted position must NOT be used here: missing
            // timestamps sort to i64::MAX, so a position comparison would
            // fabricate "missing ts = latest" and invert real ordering.
            let after = match (rec_ts, load_ts) {
                (Some(a), Some(b)) => b >= a,
                _ => li > ri,
            };
            if !after {
                continue;
            }
            consumed.insert(li);
            f.converted += 1;
            if let (Some(a), Some(b)) = (rec_ts, load_ts) {
                f.deltas_ms.push(b - a);
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
            "session_attributed_loads": self.session_attributed_loads,
            "sessions_with_loads": self.sessions_with_loads.len(),
            "loads_per_session": if self.sessions.is_empty() { Value::Null } else {
                json!((self.load_grade_loads as f64 / self.sessions.len() as f64 * 100.0).round() / 100.0)
            },
            "converted_recommendations": self.converted,
            "conversion_rate_pct": pct(self.converted, self.emitted),
            "unnecessary_loads": self.unnecessary_loads,
            // Session-scoped rate: numerator and denominator both cover
            // only session-attributed loads — unattributable loads are
            // excluded from BOTH sides rather than deflating the rate.
            "unnecessary_load_rate_pct": pct(self.unnecessary_loads, self.session_attributed_loads),
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

#[cfg(test)]
mod tests {
    use super::under_cap;

    #[test]
    fn under_cap_latches_single_notice() {
        let mut errors = Vec::new();
        let mut noted = false;
        assert!(under_cap(0, 2, "events", &mut noted, &mut errors));
        assert!(under_cap(1, 2, "events", &mut noted, &mut errors));
        // At and beyond the cap: rejected, but the notice is emitted
        // exactly once — post-cap input must not grow `errors`.
        assert!(!under_cap(2, 2, "events", &mut noted, &mut errors));
        assert!(!under_cap(2, 2, "events", &mut noted, &mut errors));
        assert!(!under_cap(999, 2, "events", &mut noted, &mut errors));
        assert_eq!(errors, vec!["events truncated at 2".to_string()]);
    }
}
