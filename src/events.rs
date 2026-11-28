//! Skill Usage Event — the common normalized observation contract shared by
//! runtime hooks, plugins, and the read-only `observe` normalizer.
//!
//! Canonical schema: `contracts/v1/skill-usage-event.schema.json`.
//! Events never carry conversation bodies, command lines, or tool output;
//! prompts are represented by `prompt_hash` (SHA-256 hex) only.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PromptSubmit,
    Recommendation,
    SkillLoad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runtime {
    ClaudeCode,
    Codex,
    Opencode,
}

impl Runtime {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedVia {
    Hook,
    Plugin,
    Transcript,
    Db,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    PromptSubmit,
    SkillLoad,
}

/// How strongly the observation supports "the skill was loaded".
/// Ordered weakest→strongest is NOT the declaration order; rank via
/// `load_rank` where needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detection {
    /// Runtime-native skill mechanism fired (e.g. Claude `Skill` tool).
    NativeInvocation,
    /// SKILL.md content was read; identity corroborated by frontmatter.
    FileRead,
    /// A SKILL.md path was referenced by a read-like access, but the
    /// content could not be verified.
    PathReference,
    /// Weaker heuristic signal (e.g. runtime skill-selection list).
    Inferred,
    /// Name/path mentioned without read evidence. Emitted by `observe`
    /// transcript scanning and by hook/plugin detection for
    /// reference-only accesses; never load-grade.
    Mention,
    /// Reserved: explicit confirmation that the skill was applied.
    /// No emitter produces this yet.
    Applied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecommendedSkill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillUsageEvent {
    pub schema_version: u32,
    pub kind: EventKind,
    pub runtime: Runtime,
    #[serde(default)]
    pub session_id: SessionId,
    #[serde(default)]
    pub timestamp: String,
    pub observed_via: ObservedVia,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub recommended: Vec<RecommendedSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommend_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection: Option<Detection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Bound on `recommended` items retained per event. Constructors cap
/// their input and the observe passthrough truncates to the same bound,
/// so a hostile payload cannot retain an unbounded name list.
pub(crate) const MAX_RECOMMENDED: usize = 32;

/// Session ids come from agent-controlled payloads and transcripts;
/// bound them like `tool`/`skill_path` so a hostile payload cannot write
/// unbounded strings into every event line. Charset matches the hook's
/// sanitizer so hook- and detect-emitted events agree.
/// Session-id contract (#225 r5/r6): the normalized form must be
/// injective over arbitrary host input, not merely collision-resistant.
/// The old scheme let a charset-valid literal id ("sess1-deadbeef")
/// equal the transformed output of a different id ("sess!1" ->
/// "sess1-deadbeef") — a structural namespace collision merging two
/// sessions' funnel state. The fix reserves the transform output
/// shape: any id ending in `-` + 32 lowercase hex chars is
/// non-canonical and is itself transformed, so passthrough and
/// generated namespaces can never overlap. Idempotency is
/// intentionally NOT a contract: for a charset-lossy mapping,
/// injectivity and idempotency are mutually exclusive (a fixed-point
/// output would have to be canonical, hence pass through, hence
/// collide with the literal that produced it).
///
/// Type-level separation (#225 r6): `SessionId` is the ONLY
/// representation of a normalized id. Raw host strings enter the
/// system exclusively through `SessionId::normalize`, which runs the
/// transform exactly once per ingest boundary (hook payload, plugin
/// event, transcript/db row). Every downstream consumer — event
/// constructors, state keys, observe passthrough — holds a
/// `SessionId` and can never re-run the transform: doing so on a
/// transform output (which ends in the reserved shape) would produce
/// a DIFFERENT id and silently split one session's funnel state.
/// Re-normalization is therefore a compile-time impossibility, not a
/// convention.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Normalize a raw host-supplied session id into contract form.
    /// This is the ONLY place the transform runs.
    pub fn normalize(session_id: &str) -> Self {
        const MAX_ID_CHARS: usize = 128;
        const HEAD_CHARS: usize = 95; // 95 + 1 + 32 = 128
        const SCAN_CHARS: usize = 8192;
        const HASH_HEX: usize = 32;

        // Canonical fast path: at most MAX_ID_CHARS+1 chars are
        // inspected, so a hostile multi-MB id cannot force a full scan
        // here. When the prefix is shorter than the cap it IS the
        // whole string.
        let prefix: Vec<char> = session_id.chars().take(MAX_ID_CHARS + 1).collect();
        if prefix.len() <= MAX_ID_CHARS
            && prefix
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
            && !has_reserved_suffix(session_id)
        {
            return SessionId(session_id.to_string());
        }

        // #225 C01: an id longer than the identity domain cannot be
        // represented injectively within the bounded work budget —
        // two distinct overlong ids agreeing on the first SCAN_CHARS
        // chars would collapse into ONE session and share its state.
        // Collapse them into the unattributed (empty) namespace
        // instead: they share no session state, and work stays
        // O(SCAN_CHARS) for any input size.
        if session_id.chars().take(SCAN_CHARS + 1).count() > SCAN_CHARS {
            return SessionId(String::new());
        }

        // Non-canonical: the identity DOMAIN is the first SCAN_CHARS
        // chars — head AND hash both derive from it. Work is
        // O(SCAN_CHARS) for ANY input size, and two raw ids that agree
        // on the domain are the same session BY CONTRACT across every
        // producer (hook, plugin, detect, observe). #225 r9: hashing
        // the full input made work O(input) while the plugin's
        // transport-side slice made the effective domain plugin-local
        // — the domain must be defined once, inside normalize.
        let domain: String = session_id.chars().take(SCAN_CHARS).collect();
        let mut head = String::new();
        let mut head_len = 0usize;
        for c in domain.chars() {
            if head_len >= HEAD_CHARS {
                break;
            }
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                head.push(c);
                head_len += 1;
            }
        }
        if head.is_empty() {
            head.push('u');
        }
        SessionId(format!(
            "{head}-{}",
            &sha256_hex(domain.as_bytes())[..HASH_HEX]
        ))
    }

    /// Adopt an already-normalized id WITHOUT re-running the
    /// transform — used at trust boundaries where the stored/emitted
    /// form is re-ingested (event-log passthrough, deserialization).
    /// Validation is charset + length only: transform outputs
    /// legitimately end in the reserved `-`+32hex shape, so the
    /// reserved suffix must NOT be rejected here. Returns None for
    /// strings that could never be normalize() output.
    pub fn from_normalized(s: &str) -> Option<Self> {
        const MAX_ID_CHARS: usize = 128;
        let mut chars = s.chars();
        if chars.by_ref().take(MAX_ID_CHARS + 1).count() > MAX_ID_CHARS {
            return None;
        }
        if s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            Some(SessionId(s.to_string()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for SessionId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for SessionId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for SessionId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl<'de> Deserialize<'de> for SessionId {
    /// Stored/emitted session ids must already be normalized; an
    /// invalid value fails the whole event deserialization so the log
    /// line is counted in `lines_skipped` rather than silently
    /// re-normalized into a different session.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        SessionId::from_normalized(&s)
            .ok_or_else(|| serde::de::Error::custom("session_id is not normalized contract form"))
    }
}

/// True when `s` ends with `-` followed by 32 lowercase hex chars —
/// the transform output shape. A longer hex tail still matches
/// (over-reserving only costs an extra transform of a literal id,
/// never a collision). Caller guarantees `s` is ASCII safe-charset,
/// so byte slicing is exact.
fn has_reserved_suffix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 33 || b[b.len() - 33] != b'-' {
        return false;
    }
    b[b.len() - 32..]
        .iter()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

impl SkillUsageEvent {
    pub fn prompt_submit(
        runtime: Runtime,
        observed_via: ObservedVia,
        session_id: &SessionId,
        timestamp: &str,
        prompt: &str,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind: EventKind::PromptSubmit,
            runtime,
            session_id: session_id.clone(),
            timestamp: timestamp.chars().take(64).collect(),
            observed_via,
            prompt_hash: Some(sha256_hex(prompt.as_bytes())),
            trigger: None,
            recommended: Vec::new(),
            emitted: None,
            reason: None,
            recommend_ms: None,
            skill_name: None,
            skill_path: None,
            skill_digest: None,
            detection: None,
            confidence: None,
            tool: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn recommendation(
        runtime: Runtime,
        observed_via: ObservedVia,
        session_id: &SessionId,
        timestamp: &str,
        prompt_hash: Option<String>,
        trigger: Trigger,
        names: Vec<RecommendedSkill>,
        emitted: bool,
        reason: &str,
        recommend_ms: Option<u64>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind: EventKind::Recommendation,
            runtime,
            session_id: session_id.clone(),
            timestamp: timestamp.chars().take(64).collect(),
            observed_via,
            prompt_hash: prompt_hash.map(|h| h.chars().take(128).collect()),
            trigger: Some(trigger),
            recommended: names
                .into_iter()
                .take(MAX_RECOMMENDED)
                .map(|mut item| {
                    item.name = item.name.chars().take(256).collect();
                    item.digest = item.digest.map(|d| d.chars().take(128).collect());
                    item
                })
                .collect(),
            emitted: Some(emitted),
            reason: Some(reason.chars().take(1024).collect()),
            recommend_ms,
            skill_name: None,
            skill_path: None,
            skill_digest: None,
            detection: None,
            confidence: None,
            tool: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn skill_load(
        runtime: Runtime,
        observed_via: ObservedVia,
        session_id: &SessionId,
        timestamp: &str,
        tool: Option<String>,
        skill_name: Option<String>,
        skill_path: Option<String>,
        detection: Detection,
        confidence: Confidence,
    ) -> Self {
        // tool/skill_path are payload-controlled; cap them so a hostile
        // payload cannot write unbounded strings into every event line.
        Self {
            schema_version: SCHEMA_VERSION,
            kind: EventKind::SkillLoad,
            runtime,
            session_id: session_id.clone(),
            timestamp: timestamp.chars().take(64).collect(),
            observed_via,
            prompt_hash: None,
            trigger: None,
            recommended: Vec::new(),
            emitted: None,
            reason: None,
            recommend_ms: None,
            skill_name,
            skill_path: skill_path.map(|p| p.chars().take(1024).collect()),
            skill_digest: None,
            detection: Some(detection),
            confidence: Some(confidence),
            tool: tool.map(|t| t.chars().take(256).collect()),
        }
    }

    /// True when the event counts as a load signal for funnel metrics.
    /// `mention` is a reference, not a load.
    pub fn counts_as_load(&self) -> bool {
        self.kind == EventKind::SkillLoad && self.detection != Some(Detection::Mention)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn prompt_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// Current time as RFC 3339 (UTC, second precision). Implemented without
/// external datetime crates to keep the dependency surface small.
pub fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    unix_secs_to_rfc3339(secs)
}

/// Milliseconds since epoch → RFC 3339.
pub fn epoch_ms_to_rfc3339(ms: i64) -> String {
    unix_secs_to_rfc3339(ms.div_euclid(1000).max(0) as u64)
}

fn unix_secs_to_rfc3339(secs: u64) -> String {
    // civil-from-days (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Extract the `name:` field from a YAML frontmatter block. Mirrors
/// `parser::split_frontmatter`: a BOM and leading blank lines are
/// tolerated, then the opening `---` fence must be the first content —
/// a `---/name:/---` block appearing later in the body is prose, not
/// frontmatter, and must not spoof the skill name. Only a bounded head
/// is scanned and fences must be `---` alone on their line.
///
/// Deliberate divergences from the canonical parser, all in the
/// conservative direction for truncated tool output: an unterminated
/// frontmatter still yields the name (runtimes routinely truncate tool
/// results mid-block, and the name line precedes the close fence), the
/// 8 KB scan window may cut a long value short, and the first `name:`
/// wins where YAML duplicate-key semantics would differ.
pub fn frontmatter_name(text: &str) -> Option<String> {
    const MAX_SCAN: usize = 8 * 1024;
    let head: String = text.chars().take(MAX_SCAN).collect();
    let head = head.strip_prefix('\u{feff}').unwrap_or(head.as_str());
    // Mirror split_frontmatter: it trims leading \n/\r chars (not just
    // blank lines), so a lone-\r opening still counts as frontmatter.
    let head = head.trim_start_matches(['\n', '\r']);
    let mut lines = head.lines();
    // Skip leading blank lines, then require the opening fence.
    let mut opened = false;
    for line in lines.by_ref() {
        if line.is_empty() {
            continue;
        }
        if line == "---" {
            opened = true;
        }
        break;
    }
    if !opened {
        return None;
    }
    for line in lines {
        // Fence must be exactly `---` on its line — `lines()` already
        // strips \n/\r, so a trailing space (`--- `) must not close a
        // fence the canonical parser (split_frontmatter) would reject.
        if line == "---" {
            return None;
        }
        if let Some(rest) = line.trim_end().strip_prefix("name:") {
            // YAML requires separation after the colon: `name:foo` is a
            // plain scalar, not a mapping — the canonical parser would
            // not see a name there, so neither may we.
            if !rest.is_empty() && !rest.starts_with(|c: char| c.is_whitespace()) {
                continue;
            }
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            // Only spec-conforming names may flow into events/state;
            // anything else is treated as absent.
            if !value.is_empty() && crate::parser::valid_skill_name(value) {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Whether `path` points at a file named exactly `SKILL.md`.
pub fn is_skill_md_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "SKILL.md")
}

/// Skill name implied by a SKILL.md path: the parent directory name.
/// Returns `None` when the directory name violates the Agent Skills
/// spec charset — an unvalidated path-derived name must never reach
/// events or LLM-facing context (prompt-injection vector).
pub fn skill_name_from_path(path: &str) -> Option<String> {
    let name = Path::new(path).parent()?.file_name()?.to_string_lossy();
    crate::parser::valid_skill_name(&name).then(|| name.into_owned())
}

/// Raw `...SKILL.md` path candidates occurring in free text, in order.
/// Candidates are maximal non-delimiter runs ending at `SKILL.md`; callers
/// must still validate with `is_skill_md_path`.
pub fn skill_md_paths(text: &str) -> Vec<String> {
    skill_md_paths_impl(text, None)
}

/// Test-visible variant returning the total bytes handed to UTF-8
/// validation, so the dense-hit regression test can assert bounded work
/// deterministically instead of relying on wall-clock timing (#225).
#[cfg(test)]
fn skill_md_paths_counting(text: &str) -> (Vec<String>, usize) {
    let mut validated = 0usize;
    let out = skill_md_paths_impl(text, Some(&mut validated));
    (out, validated)
}

fn skill_md_paths_impl(text: &str, mut validated: Option<&mut usize>) -> Vec<String> {
    const NEEDLE: &[u8] = b"SKILL.md";
    /// Unquoted path bytes: the ASCII filename-ish set plus every
    /// non-ASCII byte. UTF-8 multibyte sequences are entirely >= 0x80,
    /// so non-ASCII (e.g. Japanese) path components stay inside the run
    /// and are UTF-8 validated at the end — the old ASCII-only set
    /// truncated candidates at the first multibyte byte (#225 E07).
    fn is_path_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric()
            || b >= 0x80
            || matches!(
                b,
                b'/' | b'.'
                    | b'_'
                    | b'-'
                    | b'~'
                    | b'$'
                    | b'{'
                    | b'}'
                    | b'%'
                    | b'*'
                    | b'?'
                    | b'+'
                    | b':'
                    | b'@'
                    | b'='
                    | b','
            )
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pos = 0usize;
    const MAX_CANDIDATES: usize = 256;
    const MAX_CANDIDATE_BYTES: usize = 4096;
    // Start of the contiguous path-byte run ending at `pos`, tracked
    // forward: every byte is classified exactly once (needle bodies are
    // path bytes, so `pos = end` jumps never break the run), giving each
    // SKILL.md hit its candidate start in O(1). The previous per-hit
    // backward rescan made dense `,SKILL.md`-style input ~500x slower
    // than the input — overlapping ~4 KiB scans per 8-byte hit with no
    // retention cap able to preempt the work (#144 r22). Over-long runs
    // still reject via the length check below: the tracked start is the
    // TRUE run start, which only makes over-length candidates longer —
    // accepted candidates (<= MAX_CANDIDATE_BYTES) are identical to the
    // old floor-bounded scan.
    let mut run_start = 0usize;
    // Shell quoting state (#225 E07): inside '…' every byte except the
    // closing quote is a literal path byte (spaces included); inside
    // "…" the same, except backslash escapes the next byte. Outside
    // quotes a backslash escapes the next byte (`foo\ bar` is one path
    // component). Quote characters themselves are NOT path bytes — they
    // delimit runs, so `"dir with space/SKILL.md"` yields
    // `dir with space/SKILL.md`. Quoted command substitution
    // (`$(…)`, backticks) is out of contract: its bytes scan as ordinary
    // text.
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    // Distinct candidates and per-candidate length are both bounded: a
    // max-size payload could otherwise materialize hundreds of MB of
    // path strings before any downstream cap applies.
    while pos + NEEDLE.len() <= bytes.len() && out.len() < MAX_CANDIDATES {
        if &bytes[pos..pos + NEEDLE.len()] == NEEDLE {
            let end = pos + NEEDLE.len();
            // Forward boundary: `SKILL.mdx`, `SKILL.md.bak`, `SKILL.md-old`,
            // and `SKILL.md/…` are not the SKILL.md file. Delimiters that
            // legitimately follow a filename in prose (`,`, `)`, `"`, space)
            // are not in this set.
            if end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'.' | b'_' | b'-' | b'/'))
            {
                pos = end;
                continue;
            }
            // The needle's first byte consumed any pending escape.
            escaped = false;
            // Bound the UTF-8 validation itself: `str::len` is a byte
            // count, so `end - run_start` is the same length check
            // without materializing anything — and rejecting over-length
            // runs first keeps each hit's validation <=
            // MAX_CANDIDATE_BYTES. Running from_utf8 on the (possibly
            // run-length) slice BEFORE the length check made dense hits
            // inside one long path-byte run quadratic (#225).
            if end - run_start <= MAX_CANDIDATE_BYTES {
                if let Some(c) = validated.as_mut() {
                    **c += end - run_start;
                }
                if let Ok(candidate) = std::str::from_utf8(&bytes[run_start..end])
                    && !candidate.is_empty()
                    && seen.insert(candidate)
                {
                    out.push(candidate.to_string());
                }
            }
            pos = end;
            continue;
        }
        let b = bytes[pos];
        if escaped {
            // The escaped byte is a literal path byte — the run
            // continues and no quote/escape state applies to it.
            escaped = false;
            pos += 1;
            continue;
        }
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                    run_start = pos + 1;
                } else if q == b'"' && b == b'\\' {
                    escaped = true;
                }
                // Every other byte inside quotes — space, the other
                // quote char, non-ASCII — is a literal path byte.
                pos += 1;
            }
            None => {
                if b == b'\\' {
                    escaped = true;
                } else if b == b'\'' || b == b'"' {
                    quote = Some(b);
                    run_start = pos + 1;
                } else if !is_path_byte(b) {
                    run_start = pos + 1;
                }
                pos += 1;
            }
        }
    }
    out
}

/// How a shell command segment treats a SKILL.md reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// A read-like verb accesses the file (content likely enters context).
    Read,
    /// Non-read reference (grep/find/ls/copy/edit/etc.).
    Reference,
}

const READ_VERBS: &[&str] = &[
    "bat", "batcat", "cat", "head", "tail", "less", "more", "most", "sed", "awk", "gawk", "mawk",
    "nawk", "perl", "ruby", "python", "python2", "python3", "node", "bun", "deno", "php", "lua",
    "julia", "R", "bash", "sh", "zsh", "fish", "jq", "yq", "xxd", "od", "hexdump", "strings",
    "sort", "uniq", "cut", "tr", "fold", "fmt", "nl", "column", "tac", "rev", "view", "splain",
];

const TRANSPARENT_PREFIXES: &[&str] = &[
    "sudo", "env", "time", "nice", "nohup", "command", "builtin", "xargs", "exec", "doas",
];

/// Split a command line into segments on unquoted `&`, `;`, `|`, and
/// newlines. Single/double quotes and backslash escapes are honored so
/// a delimiter inside `"…"` does not split a segment — `echo "a;b";
/// cat x/SKILL.md` must keep the `cat` segment intact for read-verb
/// detection (#225 E07). This is a deliberately small shell subset:
/// `$()`, backticks, and here-docs are not interpreted.
fn split_shell_segments(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                } else if q == b'"' && b == b'\\' {
                    escaped = true;
                }
            }
            None => match b {
                b'\\' => escaped = true,
                b'\'' | b'"' => quote = Some(b),
                b'&' | b';' | b'|' | b'\n' => {
                    out.push(&command[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    out.push(&command[start..]);
    out
}

/// Classify how `command` accesses SKILL.md files. Returns every
/// `(path, access)` pair — each command segment is classified on its own
/// read verbs, so `cat a/SKILL.md; grep b/SKILL.md` yields a Read for `a`
/// and a Reference for `b`. Empty when no SKILL.md path is referenced.
///
/// Contract (#225 E07): POSIX-shell-ish tokenization — segments split
/// on unquoted `& ; |` and newlines; `'…'`/`"…"` quoting and `\`
/// escapes are honored for both segmentation and path extraction;
/// non-ASCII (UTF-8) path components are supported. Command
/// substitution, here-docs, and glob expansion are not interpreted —
/// their text scans as ordinary arguments.
pub fn classify_command(command: &str) -> Vec<(String, Access)> {
    let mut out: Vec<(String, Access)> = Vec::new();
    // path -> index into `out`; keeps dedup O(1) per candidate so a
    // command stuffed with distinct SKILL.md paths cannot go quadratic.
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // path -> segment index of the access that determined its strength.
    // A path first mentioned by `ls` but read later (`ls a; cat b; cat a`)
    // is ordered by its READ segment, not its first mention — tool output
    // follows command order, so this is the order files' contents appear.
    let mut decisive: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // Bound the retained access list: events per payload are capped at
    // MAX_EVENTS_PER_PAYLOAD, so accumulating more classifications than
    // that can never change the emitted output.
    const MAX_COMMAND_ACCESSES: usize = MAX_EVENTS_PER_PAYLOAD * 4;
    for (seq, segment) in split_shell_segments(command).into_iter().enumerate() {
        if out.len() >= MAX_COMMAND_ACCESSES {
            break;
        }
        // Cheap reject before allocating skill_md_paths' buffers: most
        // segments in a large command never mention SKILL.md.
        if !segment.contains("SKILL.md") {
            continue;
        }
        let access = if segment_has_read_verb(segment) {
            Access::Read
        } else {
            Access::Reference
        };
        for path in skill_md_paths(segment) {
            if !is_skill_md_path(&path) {
                continue;
            }
            // Same path seen earlier: keep the stronger access so
            // `ls x/SKILL.md && cat x/SKILL.md` still counts as a Read.
            match index.get(&path) {
                Some(&i) => {
                    if access == Access::Read && out[i].1 == Access::Reference {
                        out[i].1 = Access::Read;
                        decisive.insert(path, seq);
                    }
                }
                None => {
                    index.insert(path.clone(), out.len());
                    decisive.insert(path.clone(), seq);
                    out.push((path, access));
                }
            }
        }
    }
    // Order by decisive-access position (stable: same-segment paths keep
    // argument order). Consumers that attribute a single tool output to
    // the first-read file depend on this matching emission order.
    out.sort_by_key(|(path, _)| decisive.get(path).copied().unwrap_or(usize::MAX));
    out
}

fn segment_has_read_verb(segment: &str) -> bool {
    let mut words = segment.split_whitespace().peekable();
    let verb = loop {
        // Skip leading VAR=value assignments.
        while let Some(w) = words.peek() {
            let w = w.trim_matches(|c| c == '"' || c == '\'');
            if w.contains('=') && !w.starts_with('=') {
                words.next();
            } else {
                break;
            }
        }
        let w = match words.next() {
            Some(w) => {
                w.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == '`' || c == '$')
            }
            None => return false,
        };
        // Skip flags interleaved with transparent wrappers
        // (`sudo -E cat`, `xargs -I{} cat`). Flag *values* are not
        // skipped — `sudo -u root cat` still resolves `root` as the
        // verb; that residual is accepted (documented heuristic).
        if w.len() > 1 && w.starts_with('-') {
            continue;
        }
        // Skip transparent wrappers (sudo, env, xargs, ...); `xargs cat`
        // style still resolves to the inner verb.
        if TRANSPARENT_PREFIXES.contains(&w) {
            continue;
        }
        break w.rsplit('/').next().unwrap_or(w);
    };
    READ_VERBS.contains(&verb)
}

/// `true` when the tool looks like it reads file content rather than
/// executing a shell command. Used to decide whether `input.path`-style
/// fields should be treated as reads.
fn is_read_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "read" | "view" | "notebook_read" | "read_file"
    )
}

fn is_command_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "bash" | "shell" | "exec" | "exec_command" | "run_terminal_cmd" | "terminal" | "powershell"
    )
}

/// Classify a structured tool access (hook payload or transcript tool item),
/// gated on the tool name so that e.g. a `Write` to SKILL.md is not a load.
///
/// `tool` is the runtime tool name; `input` its arguments object; `output`
/// the captured tool output text when available (may be empty).
/// Returns every distinct skill access — a command touching several
/// SKILL.md files yields one classification per file.
pub fn classify_tool_access_all(
    tool: &str,
    input: &Value,
    output: &str,
) -> Vec<LoadClassification> {
    let lower = tool.to_ascii_lowercase();
    if lower == "skill" {
        let raw = input
            .get("skill")
            .or_else(|| input.get("name"))
            .or_else(|| input.get("skill_name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return vec![LoadClassification {
            skill_name: normalize_skill_name(raw),
            skill_path: None,
            detection: Detection::NativeInvocation,
            confidence: Confidence::High,
        }];
    }
    if is_read_tool(&lower) {
        for key in [
            "path",
            "file_path",
            "filePath",
            "notebook_path",
            "notebookPath",
        ] {
            if let Some(path) = input.get(key).and_then(Value::as_str)
                && is_skill_md_path(path)
            {
                return vec![classify_file_read(path, output)];
            }
        }
        return Vec::new();
    }
    if is_command_tool(&lower) {
        let mut out = Vec::new();
        let mut seen: std::collections::HashSet<(Option<String>, Detection)> =
            std::collections::HashSet::new();
        // One tool output can cover several SKILL.md files (`cat a b`); its
        // single frontmatter block belongs to the first file emitted, so
        // only the first Read access may claim it — attributing it to every
        // path would stamp the wrong skill name on the rest.
        let mut frontmatter_available = true;
        // Parity with detect_all_from_payload's per-payload cap: observe
        // scanners emit one event per classification, so an uncapped
        // per-use list would fan out ~(uses × ~1.5K classes) before any
        // downstream budget runs (#144 r18).
        'keys: for key in ["command", "cmd", "input"] {
            if let Some(command) = input.get(key).and_then(Value::as_str) {
                for (path, access) in classify_command(command) {
                    let c = match access {
                        Access::Read => {
                            let verify = frontmatter_available;
                            frontmatter_available = false;
                            if verify {
                                classify_file_read(&path, output)
                            } else {
                                classify_file_read_unverified(&path)
                            }
                        }
                        Access::Reference => LoadClassification {
                            skill_name: skill_name_from_path(&path),
                            skill_path: Some(path),
                            detection: Detection::Mention,
                            confidence: Confidence::Low,
                        },
                    };
                    // Unnamed classifications are dropped downstream
                    // (detect_all_from_payload emits no event for
                    // them); counting them against the cap would let
                    // 64 junk paths starve a later valid one (#144 r18
                    // CR). Frontmatter consumption above already ran,
                    // so ordering semantics are unchanged.
                    if c.skill_name.is_none() {
                        continue;
                    }
                    if out.len() >= MAX_EVENTS_PER_PAYLOAD {
                        break 'keys;
                    }
                    if seen.insert((c.skill_path.clone(), c.detection)) {
                        out.push(c);
                    }
                }
            }
        }
        return out;
    }
    Vec::new()
}

/// Single best classification for callers that can only carry one event.
/// Kept for compatibility; prefer `classify_tool_access_all`.
pub fn classify_tool_access(tool: &str, input: &Value, output: &str) -> Option<LoadClassification> {
    classify_tool_access_all(tool, input, output)
        .into_iter()
        .max_by_key(rank)
}

#[derive(Clone, Debug)]
pub struct LoadClassification {
    pub skill_name: Option<String>,
    pub skill_path: Option<String>,
    pub detection: Detection,
    pub confidence: Confidence,
}

fn classify_file_read(path: &str, output: &str) -> LoadClassification {
    if let Some(name) = frontmatter_name(output) {
        LoadClassification {
            skill_name: Some(name),
            skill_path: Some(path.to_string()),
            detection: Detection::FileRead,
            confidence: Confidence::High,
        }
    } else {
        classify_file_read_unverified(path)
    }
}

/// Path-derived classification for reads whose output cannot be tied to
/// this specific file (multi-file tool output, or no frontmatter found).
fn classify_file_read_unverified(path: &str) -> LoadClassification {
    LoadClassification {
        skill_name: skill_name_from_path(path),
        skill_path: Some(path.to_string()),
        detection: Detection::PathReference,
        confidence: Confidence::Medium,
    }
}

/// Normalize a skill name: strip namespace/path prefixes (`plugin:foo/bar`
/// -> `bar`). Returns `None` for names outside the Agent Skills spec
/// charset — they must never reach events, dedup state, or `.recommended`.
pub fn normalize_skill_name(raw: &str) -> Option<String> {
    let name = raw.rsplit(['/', ':']).next().filter(|s| !s.is_empty())?;
    crate::parser::valid_skill_name(name).then(|| name.to_string())
}

/// Extract `exec_command({cmd: "..."})` command strings from Codex `exec`
/// tool JS source. Falls back to treating recognizable shell-looking
/// fragments conservatively (returns empty when nothing parses).
pub fn extract_exec_commands(js: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = js.as_bytes();
    let mut pos = 0usize;
    // A payload can repeat `exec_command` millions of times; bound the
    // number of occurrences inspected (events are capped separately).
    const MAX_OCCURRENCES: usize = 256;
    let mut occurrences = 0usize;
    while occurrences < MAX_OCCURRENCES
        && let Some(found) = find_subslice(&bytes[pos..], b"exec_command")
    {
        occurrences += 1;
        let base = pos + found;
        // Look for `cmd` then `:` then a quoted string within a window.
        let window_end = (base + 4096).min(bytes.len());
        let Some(window) = js.get(base..window_end) else {
            pos = base + "exec_command".len();
            continue;
        };
        if let Some(cmd) = extract_cmd_param(window)
            && !cmd.trim().is_empty()
            && seen.insert(cmd.clone())
        {
            out.push(cmd);
        }
        pos = base + "exec_command".len();
    }
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Within a window starting at `exec_command`, find `cmd:`/`cmd =` followed
/// by a single- or double-quoted string; return its unescaped contents.
fn extract_cmd_param(window: &str) -> Option<String> {
    let bytes = window.as_bytes();
    // Jump between `cmd` occurrences with memcmp-speed search instead of
    // a per-byte scan; identifier-boundary checks run only on hits.
    let mut search_from = 0usize;
    while search_from + 3 <= bytes.len() {
        let rel = find_subslice(&bytes[search_from..], b"cmd")?;
        let pos = search_from + rel;
        // `cmd` must start at an identifier boundary so `{mycmd: ...}` or
        // `subcmd=` is not mistaken for the command parameter.
        let boundary = pos == 0
            || !(bytes[pos - 1].is_ascii_alphanumeric() || matches!(bytes[pos - 1], b'_' | b'$'));
        if !boundary {
            search_from = pos + 3;
            continue;
        }
        let mut i = pos + 3;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i < bytes.len() && (bytes[i] == b':' || bytes[i] == b'=') {
            i += 1;
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let start = i;
                let mut result = String::new();
                while i < bytes.len() {
                    let b = bytes[i];
                    if b == quote {
                        if start == i && result.is_empty() {
                            return Some(String::new());
                        }
                        result.push_str(&window[start..i]);
                        return Some(unescape_js(&result));
                    }
                    if b == b'\\' && i + 1 < bytes.len() {
                        // Keep escape sequences verbatim-ish: copy both
                        // bytes; unescape_js handles common cases.
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                // Unterminated — take what we have.
                result.push_str(&window[start..i.min(bytes.len())]);
                return Some(unescape_js(&result));
            }
        }
        search_from = pos + 3;
    }
    None
}

fn unescape_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Pull displayable text out of a tool_response / tool result payload.
/// Handles plain strings, `{content|stdout|output|text}` objects, and
/// arrays of `{type:"text",text}` items (Codex rollout output shape).
pub fn response_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                other => other
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            for key in ["content", "stdout", "output", "text", "result"] {
                if let Some(v) = map.get(key) {
                    let text = response_text(v);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// Upper bound on events emitted from a single payload. A hostile or buggy
/// tool call could reference thousands of distinct SKILL.md paths; without
/// a cap the hook would run unbounded recommend() passes and churn the
/// event log.
const MAX_EVENTS_PER_PAYLOAD: usize = 64;

/// True when a tool result marks the call as failed: `status:"error"`,
/// `success:false`, or a non-empty `error` value (null/false/"" are not
/// errors). Shared by live detection — which applies it to the whole
/// hook/plugin payload — and DB observation, which applies it to the
/// tool part's `state` object, so both paths classify the same tool
/// execution identically (#225 E06).
pub fn tool_result_failed(v: &Value) -> bool {
    v.get("status").and_then(Value::as_str) == Some("error")
        || v.get("success").and_then(Value::as_bool) == Some(false)
        || v.get("error").is_some_and(|e| {
            // The OpenCode plugin forwards `error` defensively; any
            // non-empty error value marks a failed call.
            !e.is_null() && *e != Value::Bool(false) && e.as_str() != Some("")
        })
}

/// Normalize a runtime hook/plugin payload into `skill_load` events —
/// one per distinct skill access (a command may touch several SKILL.md
/// files). Empty when the payload carries no skill-load signal.
///
/// Accepts the union of shapes emitted by the shell-hook adapters
/// (`tool_name`/`tool_input`/`tool_response`/`session_id`) and the OpenCode
/// plugin (`tool`/`input`/`result`/`sessionID`).
pub fn detect_all_from_payload(
    runtime: Runtime,
    observed_via: ObservedVia,
    payload: &Value,
) -> Vec<SkillUsageEvent> {
    // A failed tool call never loaded the skill.
    if tool_result_failed(payload) {
        return Vec::new();
    }
    let tool = payload
        .get("tool_name")
        .or_else(|| payload.get("tool"))
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let input = payload
        .get("tool_input")
        .or_else(|| payload.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    let output_value = payload
        .get("tool_response")
        .or_else(|| payload.get("result"))
        .or_else(|| payload.get("output"))
        .cloned()
        .unwrap_or(Value::Null);
    let output = response_text(&output_value);
    // Normalize the raw payload id exactly once here; every emitted
    // event carries the resulting SessionId and cannot be re-transformed.
    let session_id = SessionId::normalize(
        payload
            .get("session_id")
            .or_else(|| payload.get("sessionID"))
            .or_else(|| payload.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    let to_event = |c: LoadClassification| {
        SkillUsageEvent::skill_load(
            runtime,
            observed_via,
            &session_id,
            &rfc3339_now(),
            Some(tool.to_string()),
            c.skill_name,
            c.skill_path,
            c.detection,
            c.confidence,
        )
    };

    // Codex `exec` tool: input is JS source, not a shell command. Extract
    // exec_command cmd strings and classify every SKILL.md access.
    if tool.eq_ignore_ascii_case("exec") {
        let js = exec_source(&input);
        // Dedup by skill identity (path preferred, name otherwise) keeping
        // the strongest classification: `grep` before `cat` on the same
        // SKILL.md must not let the weaker mention shadow the verified
        // file_read, and one path must not emit two events under different
        // derived names.
        let mut classes: Vec<LoadClassification> = Vec::new();
        let mut index: std::collections::HashMap<Option<String>, usize> =
            std::collections::HashMap::new();
        // Same single-frontmatter rule as classify_tool_access_all: only
        // the first Read access may claim the output's frontmatter name.
        let mut frontmatter_available = true;
        let mut classify = |path: &str, access: &Access| -> LoadClassification {
            match access {
                Access::Read => {
                    let verify = frontmatter_available;
                    frontmatter_available = false;
                    if verify {
                        classify_file_read(path, &output)
                    } else {
                        classify_file_read_unverified(path)
                    }
                }
                Access::Reference => LoadClassification {
                    skill_name: skill_name_from_path(path),
                    skill_path: Some(path.to_string()),
                    detection: Detection::Mention,
                    confidence: Confidence::Low,
                },
            }
        };
        let mut push = |c: LoadClassification, classes: &mut Vec<LoadClassification>| {
            if c.skill_name.is_none() {
                return;
            }
            let key = c.skill_path.clone().or_else(|| c.skill_name.clone());
            // Only the first MAX_EVENTS_PER_PAYLOAD classes are ever
            // emitted; refuse new keys past that so a hostile payload
            // cannot grow the intermediate vec (upgrades to existing
            // keys still apply).
            if classes.len() >= MAX_EVENTS_PER_PAYLOAD && !index.contains_key(&key) {
                return;
            }
            match index.get(&key) {
                Some(&i) if rank(&classes[i]) >= rank(&c) => {}
                Some(&i) => classes[i] = c,
                None => {
                    index.insert(key, classes.len());
                    classes.push(c);
                }
            }
        };
        for cmd in extract_exec_commands(&js) {
            for (path, access) in classify_command(&cmd) {
                push(classify(&path, &access), &mut classes);
            }
        }
        // Fallback: unparsed JS may still reference SKILL.md in raw text.
        if classes.is_empty() && js.contains("SKILL.md") {
            for (path, access) in classify_command(&js) {
                push(classify(&path, &access), &mut classes);
            }
        }
        let mut events: Vec<SkillUsageEvent> = classes.into_iter().map(to_event).collect();
        events.truncate(MAX_EVENTS_PER_PAYLOAD);
        return events;
    }

    let mut events: Vec<SkillUsageEvent> = classify_tool_access_all(tool, &input, &output)
        .into_iter()
        .filter(|c| c.skill_name.is_some())
        .map(to_event)
        .collect();
    events.truncate(MAX_EVENTS_PER_PAYLOAD);
    events
}

/// Single-event view kept for callers that can only carry one result
/// (e.g. `skill-graph detect` prints the strongest access first).
pub fn detect_from_payload(
    runtime: Runtime,
    observed_via: ObservedVia,
    payload: &Value,
) -> Option<SkillUsageEvent> {
    detect_all_from_payload(runtime, observed_via, payload)
        .into_iter()
        .next()
}

/// Extract the JS source for a Codex `exec` tool input: a plain string,
/// a `cmd`/`command` field, or the serialized object as a last resort.
fn exec_source(input: &Value) -> String {
    if let Some(s) = input.as_str() {
        return s.to_string();
    }
    for key in ["cmd", "command", "input"] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            return s.to_string();
        }
    }
    input.to_string()
}

fn rank(c: &LoadClassification) -> u8 {
    match c.detection {
        Detection::NativeInvocation | Detection::FileRead | Detection::Applied => 4,
        Detection::PathReference => 3,
        Detection::Inferred => 2,
        Detection::Mention => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frontmatter_name_finds_name_line() {
        assert_eq!(
            frontmatter_name("---\nname: alpha-skill\ndescription: x\n---\nbody"),
            Some("alpha-skill".to_string())
        );
        assert_eq!(
            frontmatter_name("Output:\n{\"output\":\"---\\nname: beta\\n---\\n\"}"),
            None // escaped JSON is not raw frontmatter
        );
        assert_eq!(frontmatter_name("no frontmatter"), None);
        // A fence with a trailing space is not a fence — the canonical
        // parser (split_frontmatter) rejects it, so detection must too.
        assert_eq!(frontmatter_name("--- \nname: alpha\n---\n"), None);
        assert_eq!(frontmatter_name("---\t\nname: alpha\n---\n"), None);
        // A `---` block embedded in the body is not frontmatter: the
        // opening fence must be the first non-blank line, matching
        // split_frontmatter.
        assert_eq!(frontmatter_name("intro\n---\nname: late\n---\n"), None);
        assert_eq!(frontmatter_name("```\n---\nname: code\n---\n```"), None);
        // Leading blank lines are still allowed.
        assert_eq!(
            frontmatter_name("\n\n---\nname: lead\n---\n"),
            Some("lead".to_string())
        );
    }

    #[test]
    fn sanitize_session_id_distinguishes_invalid_ids() {
        // Charset-valid ids pass through unchanged.
        assert_eq!(SessionId::normalize("s-1.ok_2"), "s-1.ok_2");
        // Distinct all-invalid ids must not collapse into one funnel
        // session, while the same id stays deterministic for joins.
        let a = SessionId::normalize("!!!");
        let b = SessionId::normalize("???");
        assert!(a.starts_with("u-"));
        assert!(b.starts_with("u-"));
        assert_ne!(a, b);
        assert_eq!(a, SessionId::normalize("!!!"));
        // Empty stays empty — it marks unattributable events.
        assert_eq!(SessionId::normalize(""), "");
        // Overlong ids are truncated at 128 chars.
        assert_eq!(SessionId::normalize(&"x".repeat(200)).len(), 128);
    }

    #[test]
    fn sanitize_session_id_truncation_keeps_prefix_sharing_ids_distinct() {
        // Two ids sharing a 128-char valid prefix must not collapse into
        // one funnel session: the bounded head carries a hash suffix of
        // the original bytes (parity with the plugin's safeSessionID).
        let a = format!("{}-aaa", "x".repeat(200));
        let b = format!("{}-bbb", "x".repeat(200));
        let sa = SessionId::normalize(&a);
        let sb = SessionId::normalize(&b);
        assert_eq!(sa.len(), 128);
        assert_eq!(sb.len(), 128);
        assert_ne!(sa, sb);
        assert!(sa.starts_with(&"x".repeat(95)));
        assert!(sa.contains('-'));
        // Deterministic: the same id sanitizes identically for joins.
        assert_eq!(sa, SessionId::normalize(&a));
        // Exactly-128-char ids pass through unsuffixed.
        let exact = "y".repeat(128);
        assert_eq!(SessionId::normalize(&exact).as_str(), exact);
    }

    #[test]
    fn sanitize_session_id_identity_domain_is_first_8192_chars() {
        // #225 r9/r12: the identity domain is defined ONCE inside
        // normalize — the first SCAN_CHARS(8192) chars feed BOTH head
        // and hash. Ids WITHIN the domain (<=8192 chars total) that
        // differ inside it stay distinct for EVERY producer
        // (hook/plugin/detect/observe); work is O(8192) regardless of
        // input size.
        let p2 = "q".repeat(8100);
        assert_ne!(
            SessionId::normalize(&format!("{p2}X{}", "z".repeat(90))),
            SessionId::normalize(&format!("{p2}Y{}", "z".repeat(90))),
            "ids differing inside the domain must stay distinct"
        );
        // #225 C01: ids LONGER than the domain cannot be represented
        // injectively — two distinct overlong ids agreeing on the
        // first 8192 chars must NOT collapse into one session. They
        // become unattributed ("") and share no session state.
        let prefix = "p".repeat(9000);
        assert_eq!(SessionId::normalize(&format!("{prefix}AAA")), "");
        assert_eq!(SessionId::normalize(&format!("{prefix}BBB")), "");
        // Exactly-at-domain boundary still normalizes.
        let edge = "e".repeat(8192);
        assert_ne!(SessionId::normalize(&edge), "");
        // normalize must not scan the tail: a 1 MiB+ id still
        // resolves in O(SCAN_CHARS).
        let huge = format!("{}{}", "h".repeat(9000), "!".repeat(1 << 20));
        assert_eq!(SessionId::normalize(&huge), "");
    }

    #[test]
    fn sanitize_session_id_stripped_chars_stay_distinct() {
        // #225: lossy charset filtering must not merge distinct ids —
        // "sess!1" and "sess?1" both filtered to "sess1" before.
        let a = SessionId::normalize("sess!1");
        let b = SessionId::normalize("sess?1");
        assert_ne!(a, b);
        assert!(a.starts_with("sess1-"));
        assert!(b.starts_with("sess1-"));
        // Deterministic for joins, and a clean id keeps its bare form.
        assert_eq!(a, SessionId::normalize("sess!1"));
        assert_eq!(SessionId::normalize("sess1"), "sess1");
        // Fixed cross-implementation vector: opencode-plugin.test.ts
        // asserts the same literal — the sanitize rules are a shared
        // event contract across entrypoints.
        assert_eq!(a, "sess1-f1437b8fb22261cc21b7a104ca00e1b1");
    }

    #[test]
    fn sanitize_session_id_reserved_shape_cannot_collide() {
        // #225 r5 structural collision: a charset-valid LITERAL id equal
        // to another id's transform output must not merge their funnel
        // state. Transform outputs end in "-" + 32 lowercase hex; that
        // shape is reserved, so the literal is itself transformed.
        let transformed = SessionId::normalize("sess!1");
        let literal = transformed.clone(); // valid charset, <=128
        assert_ne!(SessionId::normalize(&literal), transformed);
        // A hand-crafted id in the reserved shape is transformed too.
        let crafted = format!("sess1-{}", "0123456789abcdef".repeat(2));
        assert_ne!(SessionId::normalize(&crafted).as_str(), crafted);
        // Canonical ids still pass through — the reservation only
        // catches the exact generated shape.
        assert_eq!(
            SessionId::normalize("sess-0123456789abcdef"),
            "sess-0123456789abcdef"
        );
    }

    #[test]
    fn sanitize_session_id_never_exceeds_contract_limit() {
        // #225 re-review: the stripped-chars suffix must not push the
        // result past the 128-char session_id contract — a 128-char
        // filtered id plus "-" + 8 hash chars emitted 137 before.
        for raw in [
            format!("{}!", "x".repeat(128)),
            format!("{}?", "x".repeat(120)),
            format!("{}!", "x".repeat(200)),
            "sess!1".to_string(),
        ] {
            let out = SessionId::normalize(&raw);
            assert!(out.len() <= 128, "{out:?} exceeds 128 chars");
            // Idempotency is intentionally NOT a contract (#225 r5):
            // injectivity and idempotency are mutually exclusive for a
            // charset-lossy map — a fixed-point output would be
            // canonical and collide with the literal that made it.
        }
        // Distinct long ids that share a 119-char filtered head still
        // separate via the original-bytes hash.
        let a = format!("{}!a", "x".repeat(128));
        let b = format!("{}!b", "x".repeat(128));
        assert_ne!(SessionId::normalize(&a), SessionId::normalize(&b));
    }

    #[test]
    fn sanitize_session_id_property_contract() {
        // Property sweep over adversarial inputs (#225 re-review): the
        // contract — ≤128 chars, charset-safe, deterministic — must
        // hold for ARBITRARY session ids, not just the hand-picked
        // cases above. Idempotency is deliberately NOT part of the
        // contract (#225 r5): it is mutually exclusive with
        // injectivity for a charset-lossy map. Deterministic LCG, no
        // external deps; mirrors the plugin-side property test in
        // opencode-plugin.test.ts.
        let mut state: u64 = 0x9e3779b97f4a7c15;
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pieces: [&str; 14] = [
            "a",
            "Z",
            "0",
            "-",
            ".",
            "_",
            "!",
            "?",
            " ",
            "/",
            "\n",
            "é",
            "\u{0}",
            "\u{1f600}",
        ];
        let mut seen: std::collections::HashMap<String, SessionId> =
            std::collections::HashMap::new();
        for i in 0..3000u32 {
            let len = (rand() % 300) as usize;
            let mut raw = String::new();
            for _ in 0..len {
                raw.push_str(pieces[(rand() as usize) % pieces.len()]);
            }
            let out = SessionId::normalize(&raw);
            assert!(out.len() <= 128, "{out:?} exceeds 128 chars for {raw:?}");
            // Charset: output is "" or [A-Za-z0-9._-] only — head,
            // "u-", and hash-suffix forms all use the safe alphabet.
            assert!(
                out.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
                "unsafe charset in {out:?} from {raw:?}"
            );
            // Deterministic. Idempotency is NOT asserted: it is
            // mutually exclusive with injectivity for this mapping
            // (#225 r5) — see sanitize_session_id's contract comment.
            assert_eq!(SessionId::normalize(&raw), out);
            if i < 1000 {
                seen.insert(raw, out);
            }
        }
        // Distinct raws must not collapse into one sanitized id —
        // hash suffixes make accidental collisions cryptographically
        // improbable across this sample.
        let outs: std::collections::HashSet<&SessionId> = seen.values().collect();
        assert_eq!(outs.len(), seen.len(), "distinct raws collapsed");
    }

    #[test]
    fn skill_md_paths_extracts_candidates() {
        let paths = skill_md_paths("cat .agents/skills/alpha-skill/SKILL.md now");
        assert_eq!(paths, vec![".agents/skills/alpha-skill/SKILL.md"]);
        assert!(skill_md_paths("ls skills/").is_empty());
        assert!(is_skill_md_path("a/b/SKILL.md"));
        assert!(!is_skill_md_path("a/b/MYSKILL.md"));
        assert!(!is_skill_md_path("a/b/SKILL.mdx"));
        // Forward boundary: SKILL.md followed by name/path characters is
        // a different file, not a SKILL.md access.
        assert!(skill_md_paths("cat x/SKILL.mdx").is_empty());
        assert!(skill_md_paths("cat x/SKILL.md.bak").is_empty());
        assert!(skill_md_paths("cat x/SKILL.md-old").is_empty());
        assert!(skill_md_paths("cat x/SKILL.md/y").is_empty());
        // Legitimate trailing delimiters still classify; a trailing `.`
        // is ambiguous with name continuation (`.bak`) and stays rejected.
        assert_eq!(
            skill_md_paths("cat x/SKILL.md, then cat y/SKILL.md"),
            vec!["x/SKILL.md".to_string(), "y/SKILL.md".to_string()]
        );
    }

    /// #144 round-12: the backward path-byte scan is bounded — a run
    /// longer than MAX_CANDIDATE_BYTES is rejected anyway, so scanning
    /// to position 0 would be quadratic on hostile input.
    #[test]
    fn skill_md_paths_bounds_backward_scan() {
        // A >4 KiB path-byte run before SKILL.md is rejected; a valid
        // candidate later in the same input still extracts.
        let hostile = format!("{}a/SKILL.md", "a".repeat(5000));
        assert!(skill_md_paths(&hostile).is_empty());
        let mixed = format!("{} /SKILL.md b/SKILL.md", "a".repeat(5000));
        assert_eq!(
            skill_md_paths(&mixed),
            vec!["/SKILL.md".to_string(), "b/SKILL.md".to_string()]
        );
    }

    /// #144 r22: dense `,SKILL.md` hits inside one long path-byte run
    /// must not re-scan ~4 KiB per hit — candidates resolve in O(1)
    /// via the forward-tracked run start. Hits near the run start
    /// still produce candidates (the whole run prefix); once the
    /// candidate exceeds MAX_CANDIDATE_BYTES every later hit rejects.
    #[test]
    fn skill_md_paths_dense_hits_stay_linear() {
        // 5000 'a's + 500 ",SKILL.md" hits: every candidate is the full
        // run prefix (>4 KiB) — all rejected, and no hit may pay a
        // run-length UTF-8 validation.
        let (paths, validated) =
            skill_md_paths_counting(&format!("{}{}", "a".repeat(5000), ",SKILL.md".repeat(500)));
        assert!(paths.is_empty());
        assert_eq!(validated, 0, "over-length hits reached UTF-8 validation");
        // Run-start hits within the bound still extract: "a"*4000 keeps
        // the first ~10 candidates <= 4096 bytes, the rest reject — and
        // only those in-bound hits may reach UTF-8 validation.
        let (paths, validated) =
            skill_md_paths_counting(&format!("{}{}", "a".repeat(4000), ",SKILL.md".repeat(500)));
        assert_eq!(paths.len(), 10, "{paths:?}");
        assert!(paths.iter().all(|p| p.starts_with(&"a".repeat(100))));
        // ~12 in-bound hits x <=4096 bytes each; the pre-fix code would
        // have validated every hit's growing prefix (~500 x ~6 KiB).
        assert!(
            validated <= 64 * 4096,
            "validated {validated} bytes for 500 hits — per-hit work unbounded"
        );
    }

    #[test]
    fn classify_command_distinguishes_read_and_reference() {
        let accesses = classify_command("cat .agents/skills/x/SKILL.md");
        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].0, ".agents/skills/x/SKILL.md");
        assert_eq!(accesses[0].1, Access::Read);
        assert_eq!(
            classify_command("grep foo .agents/skills/x/SKILL.md")[0].1,
            Access::Reference
        );
        assert_eq!(
            classify_command("find . -name SKILL.md")[0].1,
            Access::Reference
        );
        assert_eq!(
            classify_command("sed -n '1,5p' x/SKILL.md | head")[0].1,
            Access::Read
        );
        assert!(classify_command("ls .agents/skills").is_empty());
        assert_eq!(
            classify_command("sudo cat /tmp/sg/SKILL.md")[0].1,
            Access::Read
        );
        // Multiple SKILL.md paths in one command are all classified,
        // each with its own segment's access kind.
        let multi = classify_command("cat a/SKILL.md; grep x b/SKILL.md");
        assert_eq!(multi.len(), 2);
        assert_eq!(multi[0].1, Access::Read);
        assert_eq!(multi[1].1, Access::Reference);
    }

    /// #225 E07: non-ASCII (UTF-8) path components must stay inside the
    /// candidate run — the ASCII-only byte set used to truncate at the
    /// first multibyte byte.
    #[test]
    fn skill_md_paths_keeps_non_ascii_components() {
        assert_eq!(
            skill_md_paths("cat /home/ユーザー/skills/和名/SKILL.md"),
            vec!["/home/ユーザー/skills/和名/SKILL.md".to_string()]
        );
        // A non-ASCII component adjacent to ASCII still joins the run.
        assert_eq!(
            skill_md_paths("cat skills/日本語-skill/SKILL.md"),
            vec!["skills/日本語-skill/SKILL.md".to_string()]
        );
    }

    /// #225 E07: quoted paths keep their spaces; quote characters
    /// delimit the run so the candidate is the inner path.
    #[test]
    fn skill_md_paths_handles_quoted_paths() {
        assert_eq!(
            skill_md_paths("cat \"dir with space/my-skill/SKILL.md\""),
            vec!["dir with space/my-skill/SKILL.md".to_string()]
        );
        assert_eq!(
            skill_md_paths("cat 'dir with space/my-skill/SKILL.md'"),
            vec!["dir with space/my-skill/SKILL.md".to_string()]
        );
        // An escaped space keeps the run together (one candidate, not
        // split at the space). The emitted path preserves the raw
        // escape spelling — escapes govern tokenization boundaries;
        // consumers treat `skill_path` as opaque text and derive the
        // skill name from the parent directory, which is unaffected.
        assert_eq!(
            skill_md_paths("cat dir\\ with\\ space/my-skill/SKILL.md"),
            vec!["dir\\ with\\ space/my-skill/SKILL.md".to_string()]
        );
        // A `;` inside quotes must not end the segment: the `cat` after
        // it is still the read verb for the quoted path.
        let accesses = classify_command("echo \"a;b\"; cat \"x y/SKILL.md\"");
        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].0, "x y/SKILL.md");
        assert_eq!(accesses[0].1, Access::Read);
        // Quoted SKILL.md inside a non-read segment stays a reference.
        assert_eq!(
            classify_command("grep foo \"x y/SKILL.md\"")[0].1,
            Access::Reference
        );
        // Unterminated quote: the rest of the line scans as quoted
        // text — best-effort path extraction, never a panic.
        let _ = classify_command("cat \"unterminated/x/SKILL.md");
    }

    #[test]
    fn classify_tool_access_verifies_frontmatter() {
        let input = json!({"command": "cat .agents/skills/x/SKILL.md"});
        let out = "---\nname: real-name\n---\nbody";
        let c = classify_tool_access("Bash", &input, out).unwrap();
        assert_eq!(c.detection, Detection::FileRead);
        assert_eq!(c.confidence, Confidence::High);
        assert_eq!(c.skill_name.as_deref(), Some("real-name"));

        let c = classify_tool_access("Bash", &input, "command failed").unwrap();
        assert_eq!(c.detection, Detection::PathReference);
        assert_eq!(c.confidence, Confidence::Medium);
        assert_eq!(c.skill_name.as_deref(), Some("x"));

        let input = json!({"command": "grep foo x/SKILL.md"});
        let c = classify_tool_access("shell", &input, "").unwrap();
        assert_eq!(c.detection, Detection::Mention);
        assert_eq!(c.confidence, Confidence::Low);

        // Write tool to SKILL.md is not a load.
        let input = json!({"file_path": "x/SKILL.md", "content": "new"});
        assert!(classify_tool_access("Write", &input, "").is_none());
        let input = json!({"path": "x/SKILL.md"});
        let c = classify_tool_access("Read", &input, out).unwrap();
        assert_eq!(c.detection, Detection::FileRead);
    }

    #[test]
    fn native_skill_tool_is_high_confidence() {
        let input = json!({"skill": "plugin:ns/alpha-skill"});
        let c = classify_tool_access("Skill", &input, "").unwrap();
        assert_eq!(c.detection, Detection::NativeInvocation);
        assert_eq!(c.skill_name.as_deref(), Some("alpha-skill"));
    }

    #[test]
    fn extract_exec_commands_parses_js() {
        let js = "text(await tools.exec_command({cmd:\"cat .agents/skills/x/SKILL.md\"}))\n";
        let cmds = extract_exec_commands(js);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].contains("SKILL.md"));
        assert!(extract_exec_commands("text(1+1)").is_empty());
    }

    #[test]
    fn response_text_handles_shapes() {
        assert_eq!(response_text(&json!("plain")), "plain");
        assert_eq!(
            response_text(
                &json!([{"type":"input_text","text":"a"},{"type":"input_text","text":"b"}])
            ),
            "a\nb"
        );
        assert_eq!(
            response_text(&json!({"output":[{"text":"inner"}]})),
            "inner"
        );
    }

    #[test]
    fn rfc3339_conversion() {
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(epoch_ms_to_rfc3339(1793321018917), "2026-10-30T00:43:38Z");
    }

    /// #144 round-11: `name:foo` without YAML separation is a scalar,
    /// not a mapping — the canonical parser sees no name, so the
    /// transcript-side extractor must not claim one either.
    #[test]
    fn frontmatter_name_requires_yaml_separator() {
        assert_eq!(
            frontmatter_name("---\nname:foo\n---\nbody"),
            None,
            "name:foo is a scalar, not a mapping"
        );
        assert_eq!(
            frontmatter_name("---\nname: foo-bar\n---\nbody"),
            Some("foo-bar".to_string())
        );
        assert_eq!(
            frontmatter_name("---\nname:\tfoo-bar\n---\nbody"),
            Some("foo-bar".to_string()),
            "tab is valid YAML separation"
        );
    }
}
