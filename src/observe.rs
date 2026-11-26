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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
    /// Load-grade event count per runtime (`claude-code`, `codex`,
    /// `opencode`); `mention`-level detections are excluded.
    pub runtimes: BTreeMap<String, usize>,
    /// Non-fatal per-file/per-table problems (missing tables, unreadable
    /// files, unsupported schema drift).
    pub errors: Vec<String>,
    /// Set once `errors` hits MAX_OBSERVE_ERRORS; per-file failures
    /// are routed through `obs_error` so thousands of bad inputs cannot
    /// grow the vec past the documented cap (#144 r14).
    #[serde(skip)]
    pub errors_truncated: bool,
    /// Set once the retained-events cap notices truncation; further
    /// rejections stay silent so `errors` cannot grow with input size.
    #[serde(skip)]
    pub events_truncated: bool,
    #[serde(skip)]
    pub loads_truncated: bool,
    /// Cumulative estimated bytes of retained per-event and per-load
    /// detail, bounded by MAX_OBSERVE_RETAINED_BYTES. Counter maps stay
    /// accurate; only retained detail is dropped past the budget.
    #[serde(skip)]
    pub retained_bytes: usize,
    /// Set once the skill_loads name-cardinality cap notices truncation.
    #[serde(skip)]
    pub skill_loads_truncated: bool,
    /// Display length of the observable file path currently being
    /// scanned — SkillLoad.file is backfilled after the scan, so the
    /// retained-bytes estimate charges this up front instead of
    /// undercounting.
    #[serde(skip)]
    pub current_file_len: usize,
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
    let mut sessions = SessionSet::default();
    for file in files {
        obs.files_scanned += 1;
        obs.current_file_len = file.display().to_string().len();
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
            obs_error(&mut obs, format!("{}: file too large", file.display()));
            continue;
        }
        let loads_before = obs.loads.len();
        let result = if file.extension().and_then(|e| e.to_str()) == Some("db") {
            observe_db(&file, &mut obs, &mut sessions)
        } else {
            observe_jsonl(&file, &mut obs, &mut sessions)
        };
        if let Err(err) = result {
            obs_error(&mut obs, format!("{}: {err}", file.display()));
        }
        for load in obs.loads.iter_mut().skip(loads_before) {
            load.file = file.display().to_string();
        }
    }
    obs.sessions = sessions.hashes.len();
    if sessions.truncated {
        obs_error(
            &mut obs,
            "distinct session ids truncated at cardinality cap".to_string(),
        );
    }
    Ok(obs)
}

/// Distinct session-id counter behind the summary `sessions` field.
/// Only distinctness matters, so ids are hashed rather than retained —
/// a transcript line carrying a 200 MB sessionId must not pin that many
/// bytes for the whole scan (#144 r13). Cardinality is capped: past the
/// cap new ids are ignored and `truncated` is set.
#[derive(Default)]
struct SessionSet {
    hashes: std::collections::HashSet<u64>,
    truncated: bool,
}

const MAX_OBSERVE_SESSIONS: usize = 1_000_000;

impl SessionSet {
    fn insert(&mut self, id: &str) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        let h = h.finish();
        if self.hashes.contains(&h) {
            return;
        }
        if self.hashes.len() >= MAX_OBSERVE_SESSIONS {
            self.truncated = true;
            return;
        }
        self.hashes.insert(h);
    }
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
/// Directory recursion and per-scan error count are bounded too: the
/// file cap does not limit empty/unreadable directories, and a deep or
/// hostile tree could otherwise overflow the stack or grow `errors`
/// without bound.
const MAX_OBSERVE_DEPTH: usize = 64;
const MAX_OBSERVE_ERRORS: usize = 1024;
/// Cardinality bounds on traversal itself: observable-file and depth
/// caps alone leave `visited_dirs` and per-directory entry iteration
/// unbounded — a tree of empty directories or one directory with
/// millions of entries would burn CPU/memory without producing a
/// single observable (#144 r14).
const MAX_OBSERVE_DIRS: usize = 65_536;
const MAX_OBSERVE_DIR_ENTRIES: usize = 1_000_000;

/// Push an Observation error under the same cap as collection errors.
fn obs_error(obs: &mut Observation, msg: String) {
    if obs.errors.len() < MAX_OBSERVE_ERRORS {
        obs.errors.push(msg);
    } else if !obs.errors_truncated {
        obs.errors_truncated = true;
        obs.errors.push(format!(
            "further scan errors suppressed at {MAX_OBSERVE_ERRORS}"
        ));
    }
}

/// Push a collection error, capped; the overflow note latches so a
/// hostile tree cannot grow `errors` with input size.
fn note_error(errors: &mut Vec<String>, noted: &mut bool, msg: String) {
    if errors.len() < MAX_OBSERVE_ERRORS {
        errors.push(msg);
    } else if !*noted {
        *noted = true;
        errors.push(format!(
            "further scan errors suppressed at {MAX_OBSERVE_ERRORS}"
        ));
    }
}

fn collect_observables(path: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut visited_dirs = std::collections::HashSet::new();
    let mut seen_files: std::collections::HashSet<ObsKey> = std::collections::HashSet::new();
    let mut cap_noted = false;
    let mut err_noted = false;
    let mut depth_noted = false;
    let mut dir_noted = false;
    let mut entries_seen = 0usize;
    collect_observables_inner(
        path,
        &mut out,
        &mut errors,
        &mut visited_dirs,
        &mut seen_files,
        &mut cap_noted,
        &mut err_noted,
        &mut depth_noted,
        &mut dir_noted,
        &mut entries_seen,
        0,
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
    err_noted: &mut bool,
    depth_noted: &mut bool,
    dir_noted: &mut bool,
    entries_seen: &mut usize,
    depth: usize,
) {
    if depth > MAX_OBSERVE_DEPTH {
        if !*depth_noted {
            *depth_noted = true;
            note_error(
                errors,
                err_noted,
                format!("directory depth exceeds {MAX_OBSERVE_DEPTH}; deeper inputs skipped"),
            );
        }
        return;
    }
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
        note_error(
            errors,
            err_noted,
            format!("not a file or directory: {}", path.display()),
        );
        return;
    }
    // `is_dir` follows symlinks; a symlink cycle would recurse forever.
    // Canonicalize each directory and visit it at most once — this also
    // dedups bind mounts and overlapping roots.
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if visited_dirs.len() >= MAX_OBSERVE_DIRS {
        if !*dir_noted {
            *dir_noted = true;
            note_error(
                errors,
                err_noted,
                format!("directory count exceeds {MAX_OBSERVE_DIRS}; deeper inputs skipped"),
            );
        }
        return;
    }
    if !visited_dirs.insert(canonical) {
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            note_error(errors, err_noted, format!("{}: {e}", path.display()));
            return;
        }
    };
    for entry in entries {
        if out.len() >= MAX_OBSERVE_FILES {
            note_observable_cap(out.len(), errors, cap_noted);
            break;
        }
        *entries_seen += 1;
        if *entries_seen > MAX_OBSERVE_DIR_ENTRIES {
            if !*dir_noted {
                *dir_noted = true;
                note_error(
                    errors,
                    err_noted,
                    format!(
                        "directory entries exceed {MAX_OBSERVE_DIR_ENTRIES}; remaining inputs skipped"
                    ),
                );
            }
            return;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                note_error(errors, err_noted, format!("{}: {e}", path.display()));
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
            collect_observables_inner(
                &p,
                out,
                errors,
                visited_dirs,
                seen_files,
                cap_noted,
                err_noted,
                depth_noted,
                dir_noted,
                entries_seen,
                depth + 1,
            );
        } else if p.is_file()
            && matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("jsonl") | Some("db")
            )
        {
            // is_file() rejects FIFOs/devices: read_to_string on a FIFO
            // would block forever waiting for a writer.
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
        // Store the canonical path: every symlink component is resolved
        // at collection time, so the O_NOFOLLOW open in observe_jsonl
        // guards only the final component and a symlinked .jsonl is
        // still scanned (its target, deduped by (dev,ino)) — read_dir
        // order can no longer decide whether the file is seen (#144 r14).
        out.push(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    }
}

/// Transcript/DB inputs are user data and can be huge; refuse to slurp
/// anything beyond this into memory.
const MAX_OBSERVE_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Per-record JSON bound. `serde_json::Value` retains ~16-30x its input
/// size, so the 256 MiB file cap alone would let ONE crafted line
/// (`[0,0,…]` ≈ 134M elements) materialize gigabytes. Legitimate
/// transcript records are KB-scale; oversized lines are skipped.
const MAX_OBSERVE_LINE_BYTES: usize = 4 * 1024 * 1024;

fn observe_jsonl(
    file: &Path,
    obs: &mut Observation,
    sessions: &mut SessionSet,
) -> Result<(), String> {
    // The collect-time is_file()/size checks are TOCTOU: re-verify on
    // the opened fd (O_NONBLOCK + O_NOFOLLOW so a FIFO swap cannot hang
    // or redirect the read) and bound the read itself so a file that
    // grows past the cap after metadata() cannot be slurped whole.
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let handle = opts
        .open(file)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if !handle.metadata().map(|m| m.is_file()).unwrap_or(false) {
        return Err(format!("{}: not a regular file", file.display()));
    }
    let mut content = String::new();
    handle
        .take(MAX_OBSERVE_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if content.len() as u64 > MAX_OBSERVE_FILE_BYTES {
        return Err(format!("{}: file too large", file.display()));
    }
    let mut claude = ClaudeScan::default();
    let mut codex = CodexScan::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_OBSERVE_LINE_BYTES {
            obs.lines_skipped += 1;
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            obs.lines_skipped += 1;
            continue;
        };
        if let Some(sid) = line_session_id(&value) {
            sessions.insert(&sid);
        }
        if try_passthrough_event(&value, line, obs) {
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
/// `line` is the raw JSON text: deserializing the struct straight from
/// it avoids deep-cloning the already-parsed `Value` tree (~30x the
/// line's retained size) just to convert it (#144 r14).
fn try_passthrough_event(value: &Value, line: &str, obs: &mut Observation) -> bool {
    if value.get("kind").is_none() || value.get("schema_version").is_none() {
        return false;
    }
    let Ok(mut event) = serde_json::from_str::<SkillUsageEvent>(line) else {
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
    // skill_name must satisfy the same charset rule as recommended[].name:
    // constructors only emit valid_skill_name values, so a foreign log
    // line carrying anything else is schema-invalid in spirit and would
    // otherwise flow into funnel join keys and observe output.
    if event.kind == EventKind::SkillLoad
        && !event
            .skill_name
            .as_deref()
            .is_some_and(crate::parser::valid_skill_name)
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
/// Retained-detail byte budget alongside the count caps: every string
/// field is individually capped, but a contract-valid log can still pad
/// each event to ~13 KB, so 1M events would retain >10 GB (and `observe`
/// serializes the whole Observation into one String). Past the budget,
/// further detail is dropped with the same latched truncation notice.
const MAX_OBSERVE_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// Conservative estimate of one event's retained heap/serialized size.
fn event_retained_bytes(e: &SkillUsageEvent) -> usize {
    let mut n = 256usize;
    n += e.session_id.len() + e.timestamp.len();
    for s in [
        &e.prompt_hash,
        &e.reason,
        &e.skill_name,
        &e.skill_path,
        &e.skill_digest,
        &e.tool,
    ] {
        n += s.as_deref().map_or(0, str::len);
    }
    n += e
        .recommended
        .iter()
        .map(|r| r.name.len() + r.digest.as_deref().map_or(0, str::len) + 48)
        .sum::<usize>();
    n
}

fn load_retained_bytes(l: &SkillLoad) -> usize {
    96 + l.session_id.len()
        + l.skill.len()
        + l.timestamp.len()
        + l.file.len()
        + l.runtime.len()
        + l.detection.len()
}

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
            // The counter map keys on skill names that need not exist in
            // any discovered corpus, so its cardinality is bounded
            // separately from the retained detail: past the cap new
            // names stop being counted (existing names still are) and a
            // single latched notice records the truncation (#144 r13).
            const MAX_OBSERVE_SKILL_NAMES: usize = 65_536;
            if obs.skill_loads.contains_key(name) {
                *obs.skill_loads.get_mut(name).unwrap() += 1;
            } else if obs.skill_loads.len() < MAX_OBSERVE_SKILL_NAMES {
                obs.skill_loads.insert(name.clone(), 1);
            } else if !obs.skill_loads_truncated {
                obs.skill_loads_truncated = true;
                obs_error(obs, "skill_loads name cardinality truncated".to_string());
            }
            if under_cap(
                obs.loads.len(),
                MAX_OBSERVE_LOADS,
                "loads",
                &mut obs.loads_truncated,
                &mut obs.errors,
            ) {
                let load = SkillLoad {
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
                };
                // file is backfilled with the source path after the
                // scan; charge the current file's length up front so
                // the retained-bytes estimate does not undercount.
                let lb = load_retained_bytes(&load) + obs.current_file_len;
                if obs.retained_bytes.saturating_add(lb) <= MAX_OBSERVE_RETAINED_BYTES {
                    obs.retained_bytes += lb;
                    obs.loads.push(load);
                } else if !obs.loads_truncated {
                    obs.loads_truncated = true;
                    obs.errors
                        .push("loads truncated at retained-byte budget".to_string());
                }
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
        let eb = event_retained_bytes(&event);
        if obs.retained_bytes.saturating_add(eb) <= MAX_OBSERVE_RETAINED_BYTES {
            obs.retained_bytes += eb;
            obs.events.push(event);
        } else if !obs.events_truncated {
            obs.events_truncated = true;
            obs.errors
                .push("events truncated at retained-byte budget".to_string());
        }
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

/// Per-access path bound: extract_exec_commands output is bounded by
/// the 4 MiB line cap, but a retained path String per access would let
/// ~65K accesses pin hundreds of MB. Legitimate SKILL.md paths are far
/// shorter.
const MAX_SCAN_PATH_BYTES: usize = 4096;
/// Cap on one captured tool output retained for frontmatter
/// verification — frontmatter sits at the head of the output, so a
/// bounded prefix suffices.
const MAX_SCAN_OUTPUT_BYTES: usize = 256 * 1024;
/// Cardinality/byte budgets for scanner-retained aggregates. Count
/// caps alone do not bound bytes (each record can carry ~4 MiB of
/// strings before Value amplification), so every retained collection
/// also charges a shared byte budget (#144 r14).
const MAX_SCAN_ITEMS: usize = 65_536;
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;

/// Conservative retained-size estimate for a serde_json::Value tree
/// (strings + node overhead). Nesting is bounded by serde_json's
/// recursion limit, so this cannot overflow the stack.
fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Null | Value::Bool(_) => 8,
        Value::Number(_) => 16,
        Value::String(s) => s.len() + 24,
        Value::Array(a) => 24 + a.iter().map(value_bytes).sum::<usize>(),
        Value::Object(o) => {
            24 + o
                .iter()
                .map(|(k, v)| k.len() + value_bytes(v) + 16)
                .sum::<usize>()
        }
    }
}

/// Truncate a string to at most `max` bytes on a char boundary.
fn cap_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut n = max;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_string()
}

#[derive(Default)]
struct ClaudeScan {
    /// (tool_use_id, session_id, timestamp, tool name, input)
    uses: Vec<(String, String, String, String, Value)>,
    /// tool_use_id -> output text
    results: HashMap<String, String>,
    /// (session_id, timestamp, text) user prompts
    prompts: Vec<(String, String, String)>,
    /// Aggregate retained bytes across uses/results/prompts.
    retained: usize,
    uses_dropped: usize,
    results_dropped: usize,
    prompts_dropped: usize,
}

impl ClaudeScan {
    /// Charge `bytes` against the shared retained budget; false means
    /// the item must be dropped (counted by the caller).
    fn charge(&mut self, bytes: usize) -> bool {
        if self.retained.saturating_add(bytes) <= MAX_SCAN_BYTES {
            self.retained += bytes;
            true
        } else {
            false
        }
    }

    fn feed(&mut self, value: &Value) {
        let Some(raw_session) = value.get("sessionId").and_then(Value::as_str) else {
            return;
        };
        // Sanitize once per line: the raw sessionId is unbounded input
        // and is cloned into every retained use/prompt (#144 r14).
        let session_id = events::sanitize_session_id(raw_session);
        // Constructors truncate to 64 chars anyway; do it at ingest so
        // the retained copies are already bounded.
        let timestamp = cap_bytes(
            value.get("timestamp").and_then(Value::as_str).unwrap_or(""),
            64,
        );
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
                            let cost = id.len()
                                + session_id.len()
                                + timestamp.len()
                                + name.len()
                                + value_bytes(&input);
                            if self.uses.len() >= MAX_SCAN_ITEMS || !self.charge(cost) {
                                self.uses_dropped += 1;
                            } else {
                                self.uses.push((
                                    id,
                                    session_id.clone(),
                                    timestamp.clone(),
                                    name,
                                    input,
                                ));
                            }
                        }
                        Some("tool_result") => {
                            let id = item
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let text = cap_bytes(
                                &events::response_text(item.get("content").unwrap_or(&Value::Null)),
                                MAX_SCAN_OUTPUT_BYTES,
                            );
                            let cost = id.len() + text.len();
                            if self.results.len() >= MAX_SCAN_ITEMS || !self.charge(cost) {
                                self.results_dropped += 1;
                            } else {
                                self.results.insert(id, text);
                            }
                        }
                        Some("text") if is_user_line => {
                            if let Some(text) = item.get("text").and_then(Value::as_str) {
                                let text = text.to_string();
                                let cost = session_id.len() + timestamp.len() + text.len();
                                if self.prompts.len() >= MAX_SCAN_ITEMS || !self.charge(cost) {
                                    self.prompts_dropped += 1;
                                } else {
                                    self.prompts.push((
                                        session_id.clone(),
                                        timestamp.clone(),
                                        text,
                                    ));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(Value::String(text)) if is_user_line => {
                let text = text.clone();
                let cost = session_id.len() + timestamp.len() + text.len();
                if self.prompts.len() >= MAX_SCAN_ITEMS || !self.charge(cost) {
                    self.prompts_dropped += 1;
                } else {
                    self.prompts
                        .push((session_id.clone(), timestamp.clone(), text));
                }
            }
            _ => {}
        }
    }

    fn finish(self, obs: &mut Observation) {
        let dropped = self.uses_dropped + self.results_dropped + self.prompts_dropped;
        if dropped > 0 {
            obs_error(
                obs,
                format!("claude transcript records truncated at scan budgets: {dropped}"),
            );
        }
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
    /// Interned Rc<str>: a rollout's calls share the same session_id,
    /// and timestamp/call_id are bounded at ingest, so retaining them
    /// per access costs O(1) instead of cloning up-to-4MiB strings
    /// per record (#144 r14).
    call_id: Rc<str>,
    session_id: Rc<str>,
    timestamp: Rc<str>,
    /// (skill path, access kind) pairs extracted from exec JS commands
    accesses: Vec<(String, events::Access)>,
}

#[derive(Default)]
struct CodexScan {
    /// Sanitized session id, interned: PendingCall/OverflowAccess share
    /// this one allocation instead of cloning the raw (up to 4 MiB)
    /// session_id string per retained access (#144 r14).
    session_id: Rc<str>,
    pending: Vec<PendingCall>,
    /// Total `(path, access)` pairs retained across `pending` — bounded
    /// separately from the per-call MAX_CODEX_ACCESSES because the call
    /// count itself is unbounded and each retained path is an owned
    /// String copied out of the input (multi-MB amplification on a
    /// hostile rollout).
    pending_accesses: usize,
    /// Byte budget shared by every string this scanner retains:
    /// pending/overflow records, captured outputs, prompts, injected
    /// events. Count caps alone do not bound bytes (#144 r14).
    retained: usize,
    /// Compact records for accesses beyond the pending cap: the SKILL.md
    /// path is dropped but everything needed to emit the same
    /// skill_load event (minus skill_path) is kept, so runtimes /
    /// skill_loads counters and the funnel stay exact.
    overflow: Vec<OverflowAccess>,
    /// Accesses dropped beyond BOTH caps — finish() reports the
    /// residual count rather than silently losing the loads.
    overflow_dropped: usize,
    outputs: HashMap<String, String>,
    /// (session, timestamp, text)
    prompts: Vec<(Rc<str>, Rc<str>, String)>,
    prompts_dropped: usize,
    injected: Vec<SkillUsageEvent>,
    injected_dropped: usize,
    outputs_dropped: usize,
}

/// A retained skill-load access without its SKILL.md path. `first_read`
/// marks the call's first Read — the only access allowed to claim the
/// captured output's frontmatter, mirroring the pending path's
/// `frontmatter_available` rule.
struct OverflowAccess {
    session_id: Rc<str>,
    timestamp: Rc<str>,
    call_id: Rc<str>,
    access: events::Access,
    first_read: bool,
    /// Path-derived skill name; None means unattributable unless a
    /// first-read frontmatter upgrade supplies one (record_event drops
    /// nameless loads either way).
    name_hint: Option<String>,
}

/// Aggregate cap on retained pending accesses per rollout file.
/// ~64K paths of ≤4 KiB each stays well under the observe retained
/// budget's intent while covering any realistic session.
const MAX_PENDING_ACCESSES_TOTAL: usize = 65_536;
/// Second-tier cap for compact overflow records (no path Strings —
/// ~150 B/access worst case), keeping counters exact for corpora that
/// exceed the detailed pending budget.
const MAX_OVERFLOW_ACCESSES: usize = 65_536;

impl CodexScan {
    /// Charge `bytes` against the shared retained budget; false means
    /// the item must be dropped (counted by the caller).
    fn charge(&mut self, bytes: usize) -> bool {
        if self.retained.saturating_add(bytes) <= MAX_SCAN_BYTES {
            self.retained += bytes;
            true
        } else {
            false
        }
    }

    fn feed(&mut self, value: &Value) {
        // Constructors truncate timestamps to 64 chars; do it at ingest
        // so the interned Rc<str> retained per access is already small.
        let timestamp: Rc<str> = Rc::from(
            cap_bytes(
                value.get("timestamp").and_then(Value::as_str).unwrap_or(""),
                64,
            )
            .as_str(),
        );
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
                    // Sanitize once: the raw session_id can be ~4 MiB and
                    // was previously cloned into every retained access.
                    self.session_id = Rc::from(events::sanitize_session_id(id).as_str());
                }
            }
            Some("token_usage_record") => {
                if self.session_id.is_empty()
                    && let Some(id) = payload.get("session_id").and_then(Value::as_str)
                {
                    self.session_id = Rc::from(events::sanitize_session_id(id).as_str());
                }
            }
            Some("response_item") => match payload.get("type").and_then(Value::as_str) {
                Some("custom_tool_call") => {
                    let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
                    if name != "exec" {
                        return;
                    }
                    // call_id keys the outputs map and is retained per
                    // access; bound it so a crafted id cannot amplify.
                    let call_id: Rc<str> = Rc::from(
                        cap_bytes(
                            payload.get("call_id").and_then(Value::as_str).unwrap_or(""),
                            1024,
                        )
                        .as_str(),
                    );
                    let input = payload.get("input").and_then(Value::as_str).unwrap_or("");
                    // Dedup by path keeping the stronger access so one
                    // call touching the same SKILL.md twice counts once.
                    // The retained position is the DECISIVE access's
                    // sequence number: upgrading Reference -> Read must
                    // move the entry to the read's position or finish()
                    // would attribute the output frontmatter to a file
                    // whose content appears later (same contract as
                    // events::classify_command's decisive ordering).
                    // HashMap index keeps this O(1) per access — a hostile
                    // transcript could otherwise force quadratic scans.
                    // Access accumulation is bounded: only the first
                    // MAX_CODEX_ACCESSES accesses per call are retained.
                    const MAX_CODEX_ACCESSES: usize = 2048;
                    let mut deduped: Vec<(String, events::Access, usize)> = Vec::new();
                    let mut dedup_index: std::collections::HashMap<String, usize> =
                        std::collections::HashMap::new();
                    'accesses: for (seq, cmd) in
                        events::extract_exec_commands(input).iter().enumerate()
                    {
                        for (path, access) in events::classify_command(cmd) {
                            if deduped.len() >= MAX_CODEX_ACCESSES {
                                break 'accesses;
                            }
                            let path = cap_bytes(&path, MAX_SCAN_PATH_BYTES);
                            match dedup_index.get(&path) {
                                Some(&i)
                                    if access == events::Access::Read
                                        && deduped[i].1 == events::Access::Reference =>
                                {
                                    deduped[i].1 = events::Access::Read;
                                    deduped[i].2 = seq;
                                }
                                Some(_) => {}
                                None => {
                                    dedup_index.insert(path.clone(), deduped.len());
                                    deduped.push((path, access, seq));
                                }
                            }
                        }
                    }
                    // Fallback: unparsed JS may still reference SKILL.md in
                    // the raw input text.
                    if deduped.is_empty() && input.contains("SKILL.md") {
                        for (seq, (path, access)) in
                            events::classify_command(input).into_iter().enumerate()
                        {
                            if deduped.len() >= MAX_CODEX_ACCESSES {
                                break;
                            }
                            let path = cap_bytes(&path, MAX_SCAN_PATH_BYTES);
                            match dedup_index.get(&path) {
                                Some(&i)
                                    if access == events::Access::Read
                                        && deduped[i].1 == events::Access::Reference =>
                                {
                                    deduped[i].1 = events::Access::Read;
                                    deduped[i].2 = seq;
                                }
                                Some(_) => {}
                                None => {
                                    dedup_index.insert(path.clone(), deduped.len());
                                    deduped.push((path, access, seq));
                                }
                            }
                        }
                    }
                    deduped.sort_by_key(|&(_, _, seq)| seq);
                    let mut deduped: Vec<(String, events::Access)> =
                        deduped.into_iter().map(|(p, a, _)| (p, a)).collect();
                    if !deduped.is_empty() {
                        // Split at the remaining pending budget: the
                        // prefix keeps full path detail; the suffix is
                        // retained compactly so runtimes/skill_loads
                        // counters and the funnel still see every load.
                        let first_read_pos =
                            deduped.iter().position(|(_, a)| *a == events::Access::Read);
                        let room = MAX_PENDING_ACCESSES_TOTAL.saturating_sub(self.pending_accesses);
                        let fit = deduped.len().min(room);
                        // Byte budget: pending paths dominate retained
                        // cost. If the shared budget cannot cover them,
                        // demote the whole call to compact overflow
                        // records rather than silently dropping loads.
                        let pending_bytes = deduped[..fit]
                            .iter()
                            .map(|(p, _)| p.len() + 48)
                            .sum::<usize>()
                            + self.session_id.len()
                            + timestamp.len()
                            + call_id.len();
                        let pending_n = if fit > 0 && self.charge(pending_bytes) {
                            fit
                        } else {
                            0
                        };
                        let rest = if pending_n > 0 {
                            deduped.split_off(pending_n)
                        } else {
                            std::mem::take(&mut deduped)
                        };
                        if pending_n > 0 {
                            self.pending_accesses += pending_n;
                            self.pending.push(PendingCall {
                                call_id: call_id.clone(),
                                session_id: self.session_id.clone(),
                                timestamp: timestamp.clone(),
                                accesses: deduped,
                            });
                        }
                        for (k, (path, access)) in rest.iter().enumerate() {
                            if self.overflow.len() >= MAX_OVERFLOW_ACCESSES {
                                self.overflow_dropped += 1;
                                continue;
                            }
                            let name_hint = events::skill_name_from_path(path);
                            // name_hint is derived from the (already
                            // capped) path; charge a flat record cost.
                            if !self.charge(256 + name_hint.as_deref().map_or(0, str::len)) {
                                self.overflow_dropped += 1;
                                continue;
                            }
                            self.overflow.push(OverflowAccess {
                                session_id: self.session_id.clone(),
                                timestamp: timestamp.clone(),
                                call_id: call_id.clone(),
                                access: *access,
                                first_read: Some(pending_n + k) == first_read_pos,
                                name_hint,
                            });
                        }
                    }
                }
                Some("custom_tool_call_output") => {
                    // Same call_id bound as custom_tool_call so the
                    // outputs key matches the interned PendingCall id.
                    let call_id = cap_bytes(
                        payload.get("call_id").and_then(Value::as_str).unwrap_or(""),
                        1024,
                    );
                    // Frontmatter verification only reads the head of
                    // the output; bound the retained copy.
                    let text = cap_bytes(&codex_output_text(payload), MAX_SCAN_OUTPUT_BYTES);
                    if !call_id.is_empty() && !text.is_empty() {
                        let cost = call_id.len() + text.len();
                        if self.outputs.len() >= MAX_SCAN_ITEMS || !self.charge(cost) {
                            self.outputs_dropped += 1;
                        } else {
                            self.outputs.insert(call_id, text);
                        }
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
                            // Prompt text is hashed in full at emit
                            // time (prompt_hash must match the hash the
                            // hook/plugin computed on the untruncated
                            // prompt), so it cannot be capped — only
                            // budgeted. session_id/timestamp are shared
                            // Rc<str>s already paid for by the call
                            // records; charge the per-record text.
                            if self.prompts.len() >= MAX_SCAN_ITEMS
                                || !self.charge(timestamp.len() + text.len())
                            {
                                self.prompts_dropped += 1;
                            } else {
                                self.prompts.push((
                                    self.session_id.clone(),
                                    timestamp.clone(),
                                    text,
                                ));
                            }
                        }
                        "developer" if text.starts_with("skill-graph:") => {
                            if let Some(event) =
                                parse_injected_recommendation(&self.session_id, &timestamp, &text)
                            {
                                // recommended names are already capped
                                // by the constructor; charge a flat
                                // estimate for the event struct.
                                if self.injected.len() >= MAX_SCAN_ITEMS || !self.charge(2048) {
                                    self.injected_dropped += 1;
                                } else {
                                    self.injected.push(event);
                                }
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
        // Emit in encounter (timestamp) order, not collection order:
        // injected recommendations, prompts, and exec loads are gathered
        // from different payload types, and downstream funnel logic
        // falls back to vector order when a timestamp is unparseable.
        // A stable sort keeps same-timestamp events in feed order.
        let mut events: Vec<SkillUsageEvent> = Vec::new();
        events.extend(self.injected);
        for (session, ts, text) in &self.prompts {
            events.push(SkillUsageEvent::prompt_submit(
                Runtime::Codex,
                ObservedVia::Transcript,
                session,
                ts,
                text,
            ));
        }
        // A captured tool output's frontmatter belongs to exactly one
        // file: the first Read across ALL records sharing a call_id —
        // pending and overflow alike — may claim it. Crafted rollouts
        // can repeat call_ids; without this each would verify the same
        // output into a separate file_read event (#144 r13).
        let mut frontmatter_consumed: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        for pending in &self.pending {
            let call_id: &str = &pending.call_id;
            let output = self.outputs.get(call_id).map(String::as_str).unwrap_or("");
            // One captured output can cover several SKILL.md reads; its
            // frontmatter block belongs to the first file emitted, so only
            // the first Read access may claim it.
            let mut frontmatter_available = true;
            for (path, access) in &pending.accesses {
                let c = match access {
                    events::Access::Read => {
                        let verify =
                            frontmatter_available && !frontmatter_consumed.contains(call_id);
                        frontmatter_available = false;
                        if verify {
                            frontmatter_consumed.insert(call_id);
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
                events.push(SkillUsageEvent::skill_load(
                    Runtime::Codex,
                    ObservedVia::Transcript,
                    &pending.session_id,
                    &pending.timestamp,
                    Some("exec".to_string()),
                    c.skill_name,
                    c.skill_path,
                    c.detection,
                    c.confidence,
                ));
            }
        }
        // Overflow accesses lost their path detail but not their load:
        // emit the same event shape minus skill_path so counters, the
        // funnel, and the retained-events budget treat them identically.
        for ov in &self.overflow {
            // Only the call's first Read may claim the captured output's
            // frontmatter — same rule as the pending path, shared across
            // duplicate call_ids via `frontmatter_consumed`.
            let call_id: &str = &ov.call_id;
            let may_claim = ov.first_read && !frontmatter_consumed.contains(call_id);
            let output = if may_claim {
                self.outputs.get(call_id).map(String::as_str).unwrap_or("")
            } else {
                ""
            };
            let (name, detection, confidence) = match ov.access {
                events::Access::Read if may_claim => {
                    frontmatter_consumed.insert(call_id);
                    match events::frontmatter_name(output) {
                        Some(n) => (Some(n), Detection::FileRead, Confidence::High),
                        None => (
                            ov.name_hint.clone(),
                            Detection::PathReference,
                            Confidence::Medium,
                        ),
                    }
                }
                events::Access::Read => (
                    ov.name_hint.clone(),
                    Detection::PathReference,
                    Confidence::Medium,
                ),
                events::Access::Reference => {
                    (ov.name_hint.clone(), Detection::Mention, Confidence::Low)
                }
            };
            events.push(SkillUsageEvent::skill_load(
                Runtime::Codex,
                ObservedVia::Transcript,
                &ov.session_id,
                &ov.timestamp,
                Some("exec".to_string()),
                name,
                None,
                detection,
                confidence,
            ));
        }
        let dropped = self.overflow_dropped
            + self.prompts_dropped
            + self.injected_dropped
            + self.outputs_dropped;
        if dropped > 0 {
            obs_error(
                obs,
                format!("codex rollout records truncated at scan budgets: {dropped}"),
            );
        }
        if self.overflow_dropped > 0 {
            obs_error(
                obs,
                format!(
                    "codex rollout accesses truncated: {} beyond retention caps",
                    self.overflow_dropped
                ),
            );
        }
        events.sort_by_cached_key(|e| rfc3339_ms(&e.timestamp).unwrap_or(i64::MAX));
        for event in events {
            record_event(event, obs);
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
    // Take the cap during iteration: `names_str` is untrusted transcript
    // text and a huge comma list would otherwise materialize a Vec far
    // beyond MAX_RECOMMENDED before the constructor truncates it.
    let names: Vec<RecommendedSkill> = names_str
        .split(',')
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && crate::parser::valid_skill_name(n))
        .take(events::MAX_RECOMMENDED)
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

fn observe_db(file: &Path, obs: &mut Observation, sessions: &mut SessionSet) -> Result<(), String> {
    // Same fd-level verification the JSONL path gets: the collect-time
    // is_file()/size checks are TOCTOU, and rusqlite opens by path with
    // no O_NOFOLLOW/O_NONBLOCK of its own. A residual swap window
    // between this check and SQLite's open remains (documented in
    // contracts.md) — narrowing it further would require an fd-based
    // SQLite open, which cannot resolve the `-wal` sibling anyway.
    {
        let mut opts = fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let handle = opts
            .open(file)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let md = handle
            .metadata()
            .map_err(|e| format!("{}: {e}", file.display()))?;
        if !md.is_file() {
            return Err(format!("{}: not a regular file", file.display()));
        }
        if md.len() > MAX_OBSERVE_FILE_BYTES {
            return Err(format!("{}: file too large", file.display()));
        }
    }
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
    // `data` is materialized via substr so an oversized cell never
    // reaches Rust whole: length(data) counts characters, substr caps
    // the transferred prefix, and the byte check below still applies.
    // A row whose data exceeds the per-record bound is skipped before
    // serde_json sees it (same contract as the JSONL line cap).
    let sql = format!(
        "SELECT session_id, type, length(data), substr(data, 1, {}), time_created FROM session_message ORDER BY session_id, {order_col}",
        MAX_OBSERVE_LINE_BYTES + 1
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("query: {e}"))?;
    for row in rows {
        // A single malformed row (NULL column, type drift) must not abort
        // the whole database scan.
        let Ok((session_id, kind, data_chars, data, created_ms)) = row else {
            obs.lines_skipped += 1;
            continue;
        };
        sessions.insert(&session_id);
        if data_chars > MAX_OBSERVE_LINE_BYTES as i64 || data.len() > MAX_OBSERVE_LINE_BYTES {
            obs.lines_skipped += 1;
            continue;
        }
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
    let mut funnel_checks = MAX_FUNNEL_PAIR_CHECKS;
    let mut out = funnel_for(&all, &mut funnel_checks).into_json();
    let mut by_runtime = serde_json::Map::new();
    for runtime in ["claude-code", "codex", "opencode"] {
        let subset: Vec<&crate::events::SkillUsageEvent> = all
            .iter()
            .copied()
            .filter(|e| e.runtime.as_str() == runtime)
            .collect();
        if !subset.is_empty() {
            by_runtime.insert(
                runtime.to_string(),
                funnel_for(&subset, &mut funnel_checks).into_json(),
            );
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
        by_session_map.insert(
            session_id,
            funnel_for(&events, &mut funnel_checks).into_json(),
        );
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
    // RFC 3339 permits lowercase 't'/'z' (section 5.6 NOTE).
    if b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || !matches!(b.get(10), Some(b'T') | Some(b't'))
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
        // 'Z'/'z' must terminate the timestamp; an offset is REQUIRED —
        // an offset-less local time is not RFC 3339 and must not be
        // silently treated as UTC.
        Some(b'Z') | Some(b'z') if i + 1 == b.len() => Some(epoch_ms),
        Some(sign @ (b'+' | b'-')) => {
            // time-numoffset requires the colon: ±hh:mm only.
            if b.get(i + 3) != Some(&b':') {
                return None;
            }
            let (off_h, off_m, end) = (num(i + 1, 2)?, num(i + 4, 2)?, i + 6);
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

/// Global bound on timestamp-fallback pair examinations across ALL
/// funnel passes in one summary() — the budget is shared so per-runtime
/// and per-session recomputation cannot multiply it.
const MAX_FUNNEL_PAIR_CHECKS: usize = 8_000_000;

/// Compute funnel metrics over a set of events (overall or one runtime).
fn funnel_for(events: &[&crate::events::SkillUsageEvent], checks_left: &mut usize) -> Funnel {
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
        session_funnel(&mut f, events, idxs, checks_left);
    }
    f
}

/// conversion / unnecessary-load / timeliness for one session's
/// timestamp-ordered event indexes.
fn session_funnel(
    f: &mut Funnel,
    events: &[&crate::events::SkillUsageEvent],
    idxs: &[usize],
    checks_left: &mut usize,
) {
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

    // Parse timestamps once: the conversion scan below must not be
    // O(recommendations × loads) — a large contract-shaped event log
    // would otherwise hang `observe --summary` (the by_session cap does
    // not bound the top-level funnel, which runs over all events).
    let timestamps: std::collections::HashMap<usize, Option<i64>> = idxs
        .iter()
        .map(|&i| (i, rfc3339_ms(&events[i].timestamp)))
        .collect();

    // Greedy conversion: each emitted recommendation converts the first
    // later load-grade load naming one of its recommendations; each load
    // converts at most one recommendation.
    //
    // The naive union-of-candidates scan is O(recommendations × loads)
    // when every recommendation names the same skill — a crafted log
    // could hang `observe --summary`. Instead each name's load list
    // (already in session order) gets a cursor: loads are consumed or
    // permanently ineligible (timestamp strictly before the current
    // rec's — rec timestamps are non-decreasing in this loop) only move
    // the cursor forward, never backward. The first eligible load per
    // name is found by a short forward scan; the rec converts the
    // minimum-position winner across its names, which is exactly the
    // first eligible element of the merged union.
    //
    // Loads failing the timestamp fallback (`li > ri` encounter-order
    // check) are NOT permanently ineligible and are re-examined by
    // later recs, so a hostile all-missing-timestamp log could still
    // force quadratic scans. A global examination budget bounds that:
    // past it, remaining recs simply do not convert (deterministic —
    // rec order is fixed).
    #[derive(Default)]
    struct NameLoads {
        list: Vec<usize>,
        cursor: usize,
    }
    let mut by_name: std::collections::HashMap<&str, NameLoads> = std::collections::HashMap::new();
    for &li in &loads {
        by_name
            .entry(events[li].skill_name.as_deref().unwrap_or(""))
            .or_default()
            .list
            .push(li);
    }
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // A recommendation with an unparseable timestamp sorts last but
    // falls back to encounter-order eligibility (`li > ri`), which can
    // still admit a load that timestamp-kills already discarded for
    // earlier recs. When any rec lacks a timestamp, disable timestamp-
    // based cursor kills for the whole session; consumed-load kills stay
    // valid and the checks_left budget bounds the extra scans (#144 r13).
    let mixed_ts = recs.iter().any(|&ri| timestamps[&ri].is_none());
    'recs: for &ri in &recs {
        if consumed.len() == loads.len() {
            break;
        }
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
        // (session position, event index) of the earliest eligible load
        // per name; the winner is the minimum position across names.
        let mut best: Option<(usize, usize)> = None;
        for name in &names {
            let Some(nl) = by_name.get_mut(*name) else {
                continue;
            };
            // Advance the cursor past loads that are dead for this and
            // every later rec: consumed, or timestamped strictly before
            // rec_ts (rec timestamps are non-decreasing). Timestamp
            // kills are unsafe when the session mixes untimestamped
            // recs — see `mixed_ts` above.
            while nl.cursor < nl.list.len() {
                let li = nl.list[nl.cursor];
                let dead = !mixed_ts
                    && match (rec_ts, timestamps[&li]) {
                        (Some(a), Some(b)) => b < a,
                        _ => false,
                    };
                if consumed.contains(&li) || dead {
                    nl.cursor += 1;
                } else {
                    break;
                }
            }
            let mut j = nl.cursor;
            while j < nl.list.len() {
                let li = nl.list[j];
                j += 1;
                *checks_left = checks_left.saturating_sub(1);
                if *checks_left == 0 {
                    break 'recs;
                }
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
                let p = pos[&li];
                if best.is_none_or(|(bp, _)| p < bp) {
                    best = Some((p, li));
                }
                break;
            }
        }
        if let Some((_, li)) = best {
            consumed.insert(li);
            f.converted += 1;
            if let (Some(a), Some(b)) = (rec_ts, timestamps[&li]) {
                f.deltas_ms.push(b - a);
            }
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
    use super::{parse_injected_recommendation, under_cap};

    /// #144 round-12: a hostile transcript line with a huge comma list
    /// must not materialize an unbounded Vec — the cap is applied during
    /// iteration, before collection.
    #[test]
    fn injected_recommendation_names_are_capped_during_parse() {
        let names: Vec<String> = (0..10_000).map(|i| format!("skill-{i:05}")).collect();
        let text = format!("skill-graph: 'q' — {}.", names.join(","));
        let ev = parse_injected_recommendation("s", "2026-01-01T00:00:00Z", &text).expect("event");
        assert_eq!(ev.recommended.len(), crate::events::MAX_RECOMMENDED);
    }

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

#[cfg(test)]
mod rfc3339_tests {
    use super::rfc3339_ms;

    #[test]
    fn rfc3339_requires_offset_and_colon() {
        // Offset-less local time is not RFC 3339 — must not parse as UTC.
        assert_eq!(rfc3339_ms("2026-01-01T00:00:00"), None);
        // Colon-less numeric offset is not RFC 3339.
        assert_eq!(rfc3339_ms("2026-01-01T00:00:00+0900"), None);
        // Lowercase t/z are permitted by RFC 3339 section 5.6 NOTE.
        assert!(rfc3339_ms("2026-01-01t00:00:00z").is_some());
        // Canonical forms still parse.
        assert!(rfc3339_ms("2026-01-01T00:00:00Z").is_some());
        assert!(rfc3339_ms("2026-01-01T09:00:00+09:00").is_some());
        assert_eq!(
            rfc3339_ms("2026-01-01T00:00:00Z"),
            rfc3339_ms("2026-01-01T09:00:00+09:00")
        );
    }
}
