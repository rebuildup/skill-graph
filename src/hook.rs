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
use std::path::{Path, PathBuf};
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
    /// True when the state dir resolved to a symlink (shared-TMPDIR
    /// planting); all state reads/writes are skipped but the event log
    /// still records observations.
    state_disabled: bool,
    event_log: PathBuf,
    state_dir: PathBuf,
    event_log_max: u64,
    /// Filesystem-safe session key for state files — always non-empty.
    /// Includes the runtime so identical session ids emitted by different
    /// runtimes (e.g. claude-code and codex) cannot share dedup or
    /// recommended-skill state (#225).
    session_id: String,
    /// Unix-only dirfd anchoring the state dir: opened once with
    /// O_NOFOLLOW|O_DIRECTORY and verified dir+euid-owned, so every
    /// state file op resolves names relative to this fd via openat/
    /// fstatat and a post-check swap of the state-dir entry in shared
    /// TMPDIR cannot redirect reads or writes (#225).
    #[cfg(unix)]
    state_fd: Option<std::fs::File>,
    /// Contract session id for emitted events: normalized once from the
    /// raw payload id via `events::SessionId::normalize`. The type makes
    /// re-normalization impossible — constructors take `&SessionId`, so
    /// a transform output (reserved `-`+32hex shape) can never be fed
    /// through the transform a second time and split the session's
    /// funnel state (#225 r6).
    event_session_id: events::SessionId,
    timestamp: String,
}

/// Absolute-path env var, or None when unset/relative/empty.
fn env_absolute_path(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().and_then(|v| {
        let p = PathBuf::from(v);
        p.is_absolute().then_some(p)
    })
}

impl HookEnv {
    fn from_payload(runtime: Runtime, payload: &Value) -> Self {
        // Env-supplied paths must be absolute — same policy as the
        // adapter's HOME/XDG handling: a relative value would resolve
        // against the hook's cwd (the project tree) and drop state/log
        // files there (#144 r13).
        let tmp = env_absolute_path("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("sessionID"))
            .or_else(|| payload.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let event_session_id = events::SessionId::normalize(session_id);
        Self {
            runtime,
            nostate: std::env::var("SKILL_GRAPH_HOOK_NOSTATE").as_deref() == Ok("1"),
            // A session id that sanitizes to empty is unattributable:
            // every such session must NOT share one "unknown" dedup or
            // recommended-skill namespace — skip state reads/writes
            // entirely while the event log still records observations
            // (#225).
            state_disabled: event_session_id.is_empty(),
            event_log: env_absolute_path("SKILL_GRAPH_EVENT_LOG")
                .unwrap_or_else(|| tmp.join("skill-graph-events.jsonl")),
            state_dir: env_absolute_path("SKILL_GRAPH_HOOK_STATE")
                .unwrap_or_else(|| tmp.join("skill-graph-hook-state")),
            event_log_max: std::env::var("SKILL_GRAPH_EVENT_LOG_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64 * 1024 * 1024),
            // State files key off the contract session id scoped by
            // runtime: distinct all-invalid payload ids (hashed apart by
            // sanitize_session_id) and cross-runtime id collisions do
            // not share one dedup namespace. An absent id disables state
            // entirely via state_disabled above (#225).
            session_id: format!("{}-{}", runtime.as_str(), event_session_id),
            #[cfg(unix)]
            state_fd: None,
            event_session_id,
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
        // Rotation must not follow symlinks: symlink_metadata reports the
        // link itself, so a planted symlink (or FIFO) at the log path is
        // never renamed — parity with the OpenCode plugin's lstatSync.
        if let Ok(meta) = std::fs::symlink_metadata(&self.event_log)
            && meta.is_file()
            && meta.len() > self.event_log_max
        {
            // v0.3.x convention: append ".1" to the full log path.
            let rotated = PathBuf::from(format!("{}.1", self.event_log.display()));
            let _ = std::fs::rename(&self.event_log, rotated);
        }
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        no_follow(&mut opts);
        let Ok(f) = opts.open(&self.event_log) else {
            return;
        };
        // Shared-TMPDIR contract (docs/specs/contracts.md): the opened fd —
        // never the path — decides writability. A foreign-owned, non-regular,
        // or multiply-linked file is skipped fail-open, and permissions are
        // tightened via fchmod on the fd so a path swap cannot redirect the
        // chmod (issue #169, audit D-01).
        if !fd_writeable(&f) {
            return;
        }
        let mut f = f;
        restrict_perms_fd(&f);
        let _ = writeln!(f, "{line}");
    }

    fn marker_exists(&self, dedup_key: &str) -> bool {
        // A planted symlink must not satisfy dedup, and a foreign-owned
        // marker must not suppress recommendations either — shared
        // TMPDIR writers verify ownership before trusting state.
        if self.nostate || self.state_disabled {
            return false;
        }
        #[cfg(unix)]
        {
            let Some(fd) = self.state_fd.as_ref() else {
                return false;
            };
            // fstatat relative to the anchored dirfd: the directory
            // entry cannot be swapped out from under the lookup (#225).
            fd_stat(fd, &self.marker_name(dedup_key)).is_some_and(|st| stat_regular_owned(&st))
        }
        #[cfg(not(unix))]
        {
            self.marker_path(dedup_key)
                .symlink_metadata()
                .map(|m| m.is_file() && meta_owned_by_euid(&m))
                .unwrap_or(false)
        }
    }

    fn touch_marker(&self, dedup_key: &str) {
        if self.nostate || self.state_disabled {
            return;
        }
        #[cfg(unix)]
        {
            let Some(fd) = self.state_fd.as_ref() else {
                return;
            };
            let Some(f) = openat_state(fd, &self.marker_name(dedup_key), true) else {
                return;
            };
            // Same shared-TMPDIR contract as log_event: refuse
            // foreign-owned, non-regular, or multiply-linked targets;
            // chmod via the fd only.
            if !fd_writeable(&f) {
                return;
            }
            restrict_perms_fd(&f);
        }
        #[cfg(not(unix))]
        {
            let path = self.marker_path(dedup_key);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).write(true);
            no_follow(&mut opts);
            let Ok(f) = opts.open(path) else {
                return;
            };
            // Same shared-TMPDIR contract as log_event: refuse foreign-owned,
            // non-regular, or multiply-linked targets; chmod via the fd only.
            if !fd_writeable(&f) {
                return;
            }
            restrict_perms_fd(&f);
        }
    }

    /// State file names are relative to the anchored state dir on unix
    /// (resolved via the dirfd, never re-resolved by path) and joined
    /// under `state_dir` elsewhere (#225).
    fn marker_name(&self, dedup_key: &str) -> String {
        // Marker names: <runtime>-<session>.<cksum(dedup_key)> — the
        // runtime prefix keeps identical session ids in different
        // runtimes from sharing dedup state (#225).
        let key_hash = cksum_decimal(dedup_key.as_bytes());
        format!("{}.{key_hash}", self.session_id)
    }

    /// Per-session record of skill names surfaced by prompt-mode
    /// recommendations (issue #128). Names only — prompt text, commands,
    /// and tool responses are never persisted here.
    fn recommended_name(&self) -> String {
        format!("{}.recommended", self.session_id)
    }

    #[cfg(not(unix))]
    fn marker_path(&self, dedup_key: &str) -> PathBuf {
        self.state_dir.join(self.marker_name(dedup_key))
    }

    #[cfg(not(unix))]
    fn recommended_path(&self) -> PathBuf {
        self.state_dir.join(self.recommended_name())
    }

    fn session_recommended(&self) -> Vec<String> {
        if self.nostate || self.state_disabled {
            return Vec::new();
        }
        // State lives under a shared TMPDIR: bound the read so a planted
        // oversized file cannot OOM the hook, and refuse symlinks.
        const MAX_STATE_BYTES: u64 = 64 * 1024;
        #[cfg(unix)]
        let f = {
            let Some(fd) = self.state_fd.as_ref() else {
                return Vec::new();
            };
            // Size/type/owner gate BEFORE open via fstatat on the
            // anchored dirfd — a planted FIFO or oversized file never
            // reaches open(2), and the directory entry cannot be swapped
            // out from under the lookup (#225).
            let Some(st) = fd_stat(fd, &self.recommended_name()) else {
                return Vec::new();
            };
            if !stat_regular_owned(&st) || st.st_size > MAX_STATE_BYTES as i64 {
                return Vec::new();
            }
            let Some(f) = openat_state(fd, &self.recommended_name(), false) else {
                return Vec::new();
            };
            // Read side of the same contract: only trust a regular file
            // owned by our euid; anything else under the shared TMPDIR
            // reads as empty (fail-open).
            if !fd_is_ours(&f) {
                return Vec::new();
            }
            f
        };
        #[cfg(not(unix))]
        let f = {
            let Ok(meta) = std::fs::symlink_metadata(self.recommended_path()) else {
                return Vec::new();
            };
            if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_STATE_BYTES {
                return Vec::new();
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true);
            no_follow(&mut opts);
            let Ok(f) = opts.open(self.recommended_path()) else {
                return Vec::new();
            };
            // Read side of the same contract: only trust a regular file owned
            // by our euid; anything else under the shared TMPDIR reads as
            // empty (fail-open).
            if !fd_is_ours(&f) {
                return Vec::new();
            }
            f
        };
        let mut buf = String::new();
        use std::io::Read;
        if f.take(MAX_STATE_BYTES + 1)
            .read_to_string(&mut buf)
            .is_err()
        {
            return Vec::new();
        }
        buf.lines()
            .filter(|l| !l.is_empty() && crate::parser::valid_skill_name(l))
            .map(str::to_string)
            .collect()
    }

    fn record_recommended(&self, names: &[String]) {
        if self.nostate || self.state_disabled || names.is_empty() {
            return;
        }
        const CAP: usize = 32;
        let mut all = self.session_recommended();
        for n in names {
            if crate::parser::valid_skill_name(n) && !all.contains(n) {
                all.push(n.clone());
            }
        }
        if all.len() > CAP {
            all.drain(..all.len() - CAP);
        }
        #[cfg(unix)]
        let f = {
            let Some(dirfd) = self.state_fd.as_ref() else {
                return;
            };
            let Some(f) = openat_state(dirfd, &self.recommended_name(), true) else {
                return;
            };
            f
        };
        #[cfg(not(unix))]
        let f = {
            let path = self.recommended_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut opts = std::fs::OpenOptions::new();
            // No O_TRUNC here: truncation at open time would destroy a
            // foreign-owned or multiply-linked file before fd_writeable can
            // refuse it. set_len(0) below does the truncate only after the
            // opened fd passed the shared-TMPDIR contract.
            opts.create(true).write(true);
            no_follow(&mut opts);
            let Ok(f) = opts.open(path) else {
                return;
            };
            f
        };
        // Same shared-TMPDIR contract as log_event: refuse foreign-owned,
        // non-regular, or multiply-linked targets; chmod via the fd only.
        if !fd_writeable(&f) {
            return;
        }
        let mut f = f;
        restrict_perms_fd(&f);
        use std::io::Write;
        if f.set_len(0).is_err() {
            return;
        }
        let _ = f.write_all((all.join("\n") + "\n").as_bytes());
    }
}

/// Open the state dir anchored: O_NOFOLLOW|O_DIRECTORY refuse a symlinked
/// or non-directory final component at open time, and the fd is verified
/// to be an euid-owned directory before use. All state file ops then go
/// through openat/fstatat relative to this fd — after the open, swapping
/// the directory entry in shared TMPDIR cannot redirect them (#225).
#[cfg(unix)]
fn open_state_dir(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::FromRawFd;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state dir path contains NUL",
        )
    })?;
    // SAFETY: `c` is a valid NUL-terminated path; flags refuse a
    // symlinked or non-directory final component. The returned fd is
    // verified before use.
    let raw = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh open file descriptor.
    let f = unsafe { std::fs::File::from_raw_fd(raw) };
    let md = f.metadata()?;
    // An attacker-precreated state dir in shared TMPDIR is
    // attacker-owned and its entry can be swapped at will — only an
    // euid-owned directory is a trustworthy anchor.
    if !md.is_dir() || md.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "state dir is not an euid-owned directory",
        ));
    }
    Ok(f)
}

/// fstatat relative to the anchored state dir — never re-resolves the
/// directory path, so the lookup cannot be redirected by swapping the
/// state-dir entry (#225). AT_SYMLINK_NOFOLLOW reports a planted link
/// itself rather than its target.
#[cfg(unix)]
fn fd_stat(dirfd: &std::fs::File, name: &str) -> Option<libc::stat> {
    use std::os::unix::io::AsRawFd;
    let c = std::ffi::CString::new(name).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dirfd` is open; `st` is a valid out-pointer; `c` is a
    // valid NUL-terminated relative name (state file names contain no
    // `/` or NUL by construction).
    let rc = unsafe {
        libc::fstatat(
            dirfd.as_raw_fd(),
            c.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    (rc == 0).then_some(st)
}

/// Regular file owned by our euid, from fstatat metadata (which never
/// follows the final symlink).
#[cfg(unix)]
fn stat_regular_owned(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG && st.st_uid == unsafe { libc::geteuid() }
}

/// openat relative to the anchored state dir with O_NOFOLLOW (and
/// O_NONBLOCK so a planted FIFO cannot hang the open) — `create`
/// selects O_CREAT with mode 0600 for the write paths.
#[cfg(unix)]
fn openat_state(dirfd: &std::fs::File, name: &str, create: bool) -> Option<std::fs::File> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let c = std::ffi::CString::new(name).ok()?;
    let flags = if create {
        libc::O_WRONLY | libc::O_CREAT
    } else {
        libc::O_RDONLY
    };
    // SAFETY: `dirfd` is open; `c` is a valid relative name; the mode
    // argument is only consulted with O_CREAT.
    let raw = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            c.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if raw < 0 {
        return None;
    }
    // SAFETY: `raw` is a fresh open file descriptor.
    Some(unsafe { std::fs::File::from_raw_fd(raw) })
}

/// fd-based permission tightening for files we write (event log, markers,
/// session state). fchmod on the already-open File means a path swap
/// between open and chmod cannot redirect the mode change — the old
/// path-based chmod had exactly that TOCTOU (issue #169, audit D-01).
/// Best-effort: state lives under a shared TMPDIR, so files must not be
/// world-readable even though contents are already minimized.
#[cfg(unix)]
fn restrict_perms_fd(f: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_perms_fd(_f: &std::fs::File) {}

/// 0700 on the state dir, applied through the already-verified dirfd:
/// fchmod on the fd can never be redirected by swapping the directory
/// entry after open — the path-based variant it replaces could be
/// aimed at an arbitrary euid-owned directory via a planted symlink
/// (#225 re-review).
#[cfg(unix)]
fn restrict_dir_fd(f: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir_perms(_path: &std::path::Path) {}

/// Post-open check for files we only read: must be a regular file owned
/// by our euid. Foreign-owned state planted in a shared TMPDIR is ignored
/// (fail-open) rather than trusted.
#[cfg(unix)]
fn fd_is_ours(f: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    f.metadata()
        .map(|m| m.is_file() && m.uid() == unsafe { libc::geteuid() })
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn fd_is_ours(_f: &std::fs::File) -> bool {
    true
}

/// Post-open check for every file we write: regular file, owned by our
/// euid, single link. `nlink > 1` means the inode is reachable through
/// another path whose lifecycle we do not control — appending would
/// modify every link, so fail open instead. This is the Rust-side
/// counterpart of the OpenCode plugin's `st.uid !== getuid()` refusal,
/// extended to non-regular files and hardlinks (issue #169).
#[cfg(unix)]
fn fd_writeable(f: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    f.metadata()
        .map(|m| m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.nlink() == 1)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn fd_writeable(_f: &std::fs::File) -> bool {
    true
}

/// Ownership check on already-fetched metadata — only used by the
/// non-unix path fallback; unix state reads go through the dirfd-anchored
/// fstatat checks instead (#225).
#[cfg(not(unix))]
fn meta_owned_by_euid(_m: &std::fs::Metadata) -> bool {
    true
}

/// Refuse to follow a symlink at the final path component. State lives
/// under a shared TMPDIR where another local user could pre-plant links;
/// O_NOFOLLOW makes open(2) fail instead of reading/writing through
/// symlinks; O_NONBLOCK keeps a planted FIFO or socket at a state path
/// from hanging the hook — for regular files both flags are no-ops.
#[cfg(unix)]
fn no_follow(opts: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

#[cfg(not(unix))]
fn no_follow(_opts: &mut std::fs::OpenOptions) {}

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

/// Runtime-specific fallback roots. `SKILL_GRAPH_ROOTS` is handled by
/// `discovery::resolve_roots` and takes precedence over these.
fn hook_fallback_roots(runtime: Runtime) -> Vec<PathBuf> {
    // A relative HOME would resolve `~`-style roots against the hook's
    // cwd (the project tree); require absolute like the adapter's env
    // policy (#144 r17).
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
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
    let discovery = discover(roots);
    // Fail-open stays, but broken roots must not vanish silently —
    // bounded stderr note (hook stderr does not reach the agent).
    for err in discovery.errors.iter().take(3) {
        crate::cli::write_stderr_line(&format!("skill-graph hook: {err}"));
    }
    let graph = build(discovery);
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
        &env.event_session_id,
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
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool.eq_ignore_ascii_case("skill") {
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
    let detected = events::detect_all_from_payload(env.runtime, ObservedVia::Hook, payload);
    if detected.is_empty() {
        return None;
    }
    // One payload may load several SKILL.md files; every access is logged
    // and each load-grade skill gets its own dependency recommendation.
    // Discovery+graph build happens once per payload, not per skill.
    let mut contexts = Vec::new();
    let mut graph_cache: Option<crate::resolver::Graph> = None;
    let mut no_roots_emitted = false;
    // Each load-grade skill triggers a full recommend() pass; cap the
    // number of recommendation calls per payload so a command stuffed
    // with SKILL.md paths cannot pin the hook. Events are still logged;
    // skipped skills simply get no dependency injection this run.
    const MAX_LOAD_RECOMMENDS: usize = 8;
    let mut recommend_budget = MAX_LOAD_RECOMMENDS;
    for event in detected {
        // detect_all_from_payload already normalized the same raw
        // payload id into a SessionId — the type guarantees every
        // event agrees with env.event_session_id (#225 r6).
        env.log_event(&event);
        let Some(detection) = event.detection else {
            continue;
        };
        if !LOAD_GRADE.contains(&detection) {
            continue;
        }
        let Some(skill_name) = event.skill_name.clone() else {
            continue;
        };
        let graph = graph_cache.get_or_insert_with(|| {
            let roots =
                crate::discovery::resolve_roots(Vec::new(), || hook_fallback_roots(env.runtime));
            let discovery = discover(&roots);
            for err in discovery.errors.iter().take(3) {
                crate::cli::write_stderr_line(&format!("skill-graph hook: {err}"));
            }
            build(discovery)
        });
        if graph.skills.is_empty() {
            // No skills discovered: emit no_roots once, keep logging any
            // remaining detected events in this payload.
            if !no_roots_emitted {
                emit_recommendation(
                    env,
                    Trigger::SkillLoad,
                    None,
                    &[],
                    false,
                    "no_roots",
                    Some(0),
                );
                no_roots_emitted = true;
            }
            continue;
        }
        if let Some(ctx) = skill_load_context(env, &skill_name, graph, &mut recommend_budget) {
            contexts.push(ctx);
        }
    }
    if contexts.is_empty() {
        return None;
    }
    Some(hook_response("PostToolUse", &contexts.join("\n")))
}

/// Dependency-recommendation flow for one loaded skill. Emits the
/// `recommendation` event for every outcome and returns the injection
/// text only when there is something to surface.
fn skill_load_context(
    env: &HookEnv,
    skill_name: &str,
    graph: &crate::resolver::Graph,
    recommend_budget: &mut usize,
) -> Option<String> {
    // Budget is checked before touching the dedup marker: a skill skipped
    // for budget reasons was never recommended, so a later payload may
    // still surface it.
    if *recommend_budget == 0 {
        return None;
    }
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

    *recommend_budget -= 1;
    // Session continuity (issue #128): names this session's prompt mode
    // already surfaced become recommendation context.
    let session_ctx = env.session_recommended().join(" ");
    let start = Instant::now();
    let recs = recommend(graph, skill_name, &session_ctx, 5);
    let recommend_ms = start.elapsed().as_millis() as u64;

    // recommended[] records exactly the set injected into context so
    // observe --summary funnel counts stay truthful.
    let injected: Vec<crate::model::Recommendation> = recs
        .iter()
        .filter(|r| r.skill.name != skill_name)
        .take(3)
        .cloned()
        .collect();
    let names: Vec<&str> = injected.iter().map(|r| r.skill.name.as_str()).collect();
    if names.is_empty() {
        emit_recommendation(
            env,
            Trigger::SkillLoad,
            None,
            &[],
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
        &injected,
        true,
        "recommended",
        Some(recommend_ms),
    );
    let context = format!(
        "skill-graph: '{skill_name}' declares dependencies — {}. Read the matching SKILL.md files if the task needs them.",
        names.join(", ")
    );
    Some(context)
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
            &env.event_session_id,
            &env.timestamp,
            &query,
        ));
    }
    let phash = events::prompt_hash(&query);
    // Empty prompts carry no signal; exit before emitting events so
    // degenerate submits cannot spam the event log.
    if query.is_empty() {
        return None;
    }
    let roots = crate::discovery::resolve_roots(Vec::new(), || hook_fallback_roots(env.runtime));
    if roots.is_empty() {
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

    // recommended[] records exactly the injected top-3 so funnel counts
    // in observe --summary match what the model actually saw.
    let injected = &recs[..recs.len().min(3)];
    let names: Vec<&str> = injected.iter().map(|r| r.skill.name.as_str()).collect();
    if names.is_empty() {
        emit_recommendation(
            env,
            Trigger::PromptSubmit,
            Some(phash.clone()),
            &[],
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
        injected,
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

/// Hook payloads are small control messages; anything near this size is
/// not a legitimate hook call and is dropped fail-open.
const MAX_STDIN_BYTES: u64 = 16 * 1024 * 1024;

/// Parsed-payload bound: Value-tree amplification makes multi-MiB JSON
/// cost hundreds of MB transiently. Legitimate hook messages are
/// KB-scale.
const MAX_PARSE_BYTES: usize = 4 * 1024 * 1024;

fn run_hook_inner(runtime: Runtime, mode: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES + 1)
        .read_to_string(&mut input)
        .map_err(|e| format!("stdin: {e}"))?;
    if input.len() as u64 > MAX_STDIN_BYTES {
        // `take` stopped at the cap with payload still in the pipe:
        // drain a bounded remainder so the caller does not hit EPIPE
        // writing to a process that already exited (same contract as
        // the invalid-args path in cli.rs).
        let mut sink = Vec::new();
        let _ = std::io::stdin()
            .take(MAX_STDIN_BYTES)
            .read_to_end(&mut sink);
        return Ok(());
    }
    // Empty, non-object, or unparseable stdin is not a hook payload:
    // exit before touching state dirs or the event log (matches the
    // v0.3.x shell hook, which exited early when jq could not parse).
    // serde_json::Value retains ~16-30x its input size, so refuse to
    // materialize a tree for payloads far beyond any legitimate hook
    // message (stdin is already drained to EOF or the cap above).
    if input.len() > MAX_PARSE_BYTES {
        return Ok(());
    }
    let payload: Value = match serde_json::from_str::<Value>(input.trim()) {
        Ok(v) if v.is_object() => v,
        _ => return Ok(()),
    };
    let mut env = HookEnv::from_payload(runtime, &payload);

    if env.nostate {
        // No state dir needed, but the event log is also skipped by log_event.
    } else {
        // State lives under a shared TMPDIR: a pre-planted symlink at the
        // state-dir path would redirect marker/session writes (and the
        // chmod below) into an arbitrary directory. Refuse state ops in
        // that case; the event log still records observations.
        let is_link = std::fs::symlink_metadata(&env.state_dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_link {
            env.state_disabled = true;
        } else {
            // Create missing dirs with 0700 up front: create_dir_all
            // would leave a umask-default (e.g. 0755) window before the
            // dirfd fchmod below, exposing state filenames to other
            // TMPDIR users (#225 re-review).
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let _ = std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(&env.state_dir);
            }
            #[cfg(not(unix))]
            let _ = std::fs::create_dir_all(&env.state_dir);
            #[cfg(unix)]
            {
                // Anchor EVERY state-dir operation — including the
                // permission tightening — to the verified dirfd. The
                // previous path-based chmod reopened the file between
                // check and fchmod, so a symlink swapped in after the
                // lstat could redirect the mode change into an
                // arbitrary euid-owned directory (#225 re-review).
                match open_state_dir(&env.state_dir) {
                    Ok(fd) => {
                        restrict_dir_fd(&fd);
                        env.state_fd = Some(fd);
                    }
                    Err(_) => env.state_disabled = true,
                }
            }
            #[cfg(not(unix))]
            restrict_dir_perms(&env.state_dir);
        }
    }

    let response = match mode {
        "skill-load" => run_skill_load(&env, &payload),
        _ => run_prompt(&env, &payload),
    };
    if let Some(out) = response {
        // EPIPE-tolerant: a consumer closing stdout early must not turn
        // the fail-open hook into a panic (exit 101).
        let _ = crate::cli::write_stdout_line(&out);
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
