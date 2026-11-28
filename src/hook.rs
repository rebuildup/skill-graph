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
//!                            (default <state-root>/events.jsonl)
//!   SKILL_GRAPH_HOOK_STATE   dedup marker dir
//!                            (default <state-root>/hook-state)
//!   <state-root>             ${XDG_STATE_HOME}/skill-graph, else
//!                            ${HOME}/.local/state/skill-graph, else
//!                            ${TMPDIR}/skill-graph — per-user by
//!                            default so shared /tmp cannot collide
//!                            state across accounts (#225 D06)
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

/// Per-user state root (#225 D06): shared TMPDIR defaults let two
/// accounts on one host collide on `skill-graph-events.jsonl` and the
/// hook-state dir. Prefer XDG/HOME so state is account-scoped; TMPDIR
/// remains only as a last resort when no home is knowable. Parity with
/// the OpenCode plugin's EVENT_LOG default.
fn per_user_state_root() -> PathBuf {
    if let Some(xdg) = env_absolute_path("XDG_STATE_HOME") {
        return xdg.join("skill-graph");
    }
    if let Some(home) = env_absolute_path("HOME") {
        return home.join(".local").join("state").join("skill-graph");
    }
    env_absolute_path("TMPDIR")
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("skill-graph")
}

impl HookEnv {
    fn from_payload(runtime: Runtime, payload: &Value) -> Self {
        // Env-supplied paths must be absolute — same policy as the
        // adapter's HOME/XDG handling: a relative value would resolve
        // against the hook's cwd (the project tree) and drop state/log
        // files there (#144 r13).
        let state_root = per_user_state_root();
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
                .unwrap_or_else(|| state_root.join("events.jsonl")),
            state_dir: env_absolute_path("SKILL_GRAPH_HOOK_STATE")
                .unwrap_or_else(|| state_root.join("hook-state")),
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
        // #225 D03: the same trust contract as the append path applies
        // BEFORE the rename — a foreign-owned or multiply-linked log is
        // never rotated aside (the subsequent open+fd_writeable would
        // refuse the append anyway, but the rename itself must not
        // disturb a file we do not own).
        let rotatable = std::fs::symlink_metadata(&self.event_log)
            .map(|meta| {
                meta.is_file() && meta.len() > self.event_log_max && log_meta_trusted(&meta)
            })
            .unwrap_or(false);
        if rotatable {
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
            fd_stat(fd, &self.marker_name(dedup_key)).is_some_and(|st| stat_trusted_state(&st))
        }
        #[cfg(not(unix))]
        {
            self.marker_path(dedup_key)
                .symlink_metadata()
                .map(|m| m.is_file() && meta_owned_by_euid(&m))
                .unwrap_or(false)
        }
    }

    /// Atomically claim the dedup marker (#225 C04/C06): returns
    /// `Some(true)` when THIS call created the marker, `Some(false)`
    /// when a trusted marker already exists (dedup hit), and `None`
    /// when the state backend failed — callers treat `None` as
    /// "proceed without dedup" (fail-open) rather than suppressing a
    /// recommendation on a transient state error. The create is
    /// O_EXCL: two concurrent hooks evaluating the same prompt cannot
    /// both win the claim, closing the check-then-create race where
    /// both would inject.
    fn try_claim_marker(&self, dedup_key: &str) -> Option<bool> {
        if self.nostate || self.state_disabled {
            return None;
        }
        let name = self.marker_name(dedup_key);
        #[cfg(unix)]
        {
            let fd = self.state_fd.as_ref()?;
            use std::os::unix::io::{AsRawFd, FromRawFd};
            let c = std::ffi::CString::new(name.as_str()).ok()?;
            // SAFETY: `fd` is open; `c` is a valid relative name; the
            // mode argument is only consulted with O_CREAT.
            let raw = unsafe {
                libc::openat(
                    fd.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_NONBLOCK
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if raw >= 0 {
                // SAFETY: `raw` is a fresh open file descriptor.
                let f = unsafe { std::fs::File::from_raw_fd(raw) };
                if !fd_writeable(&f) {
                    return None;
                }
                restrict_perms_fd(&f);
                return Some(true);
            }
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                // A marker exists: trust it only when it satisfies the
                // same contract as marker_exists (regular, euid-owned,
                // single link).
                return Some(fd_stat(fd, &name).is_some_and(|st| stat_trusted_state(&st)));
            }
            None
        }
        #[cfg(not(unix))]
        {
            let path = self.marker_path(dedup_key);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            no_follow(&mut opts);
            match opts.open(&path) {
                Ok(f) => {
                    if !fd_writeable(&f) {
                        return None;
                    }
                    restrict_perms_fd(&f);
                    Some(true)
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Some(
                    path.symlink_metadata()
                        .map(|m| {
                            m.is_file() && !m.file_type().is_symlink() && meta_owned_by_euid(&m)
                        })
                        .unwrap_or(false),
                ),
                Err(_) => None,
            }
        }
    }

    /// State file names are relative to the anchored state dir on unix
    /// (resolved via the dirfd, never re-resolved by path) and joined
    /// under `state_dir` elsewhere (#225).
    fn marker_name(&self, dedup_key: &str) -> String {
        // Marker names: <runtime>-<session>.<sha256-16(dedup_key)> —
        // the runtime prefix keeps identical session ids in different
        // runtimes from sharing dedup state, and the digest keeps
        // distinct dedup keys collision-free (#225 C05: the old POSIX
        // cksum is CRC32 — two different prompts could share a marker
        // and suppress each other's recommendations).
        let key_hash = &events::sha256_hex(dedup_key.as_bytes())[..16];
        format!("{}.{key_hash}", self.session_id)
    }

    #[cfg(not(unix))]
    fn marker_path(&self, dedup_key: &str) -> PathBuf {
        self.state_dir.join(self.marker_name(dedup_key))
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

/// Trusted state-file metadata from fstatat (which never follows the
/// final symlink): regular file, owned by our euid, single link. The
/// nlink check applies to READS too (#225 D05): a multiply-linked
/// marker is reachable through a path whose lifecycle we do not
/// control, so it must not satisfy dedup — same contract as the write
/// side's fd_writeable.
#[cfg(unix)]
fn stat_trusted_state(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
        && st.st_uid == unsafe { libc::geteuid() }
        && st.st_nlink == 1
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
fn restrict_dir_fd(f: &std::fs::File) -> bool {
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(0o700))
        .is_ok()
}

#[cfg(not(unix))]
fn restrict_dir_perms(_path: &std::path::Path) {}

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

/// Ownership/link-count trust for the event log evaluated on
/// symlink_metadata (never follows the final link) — the pre-rename
/// counterpart of fd_writeable's post-open check (#225 D03).
#[cfg(unix)]
fn log_meta_trusted(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == unsafe { libc::geteuid() } && meta.nlink() == 1
}

#[cfg(not(unix))]
fn log_meta_trusted(_meta: &std::fs::Metadata) -> bool {
    true
}

/// Path of an open fd's directory — used to list the anchored state
/// dir WITHOUT re-resolving `state_dir` (a swapped TMPDIR entry cannot
/// redirect the listing to a different directory). Linux uses
/// /proc/self/fd; macOS uses F_GETPATH; other unixes fall back to the
/// configured path (best-effort, still bounded).
#[cfg(all(unix, target_os = "linux"))]
fn fd_path(fd: &std::fs::File) -> Option<PathBuf> {
    use std::os::unix::io::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).ok()
}

#[cfg(all(unix, target_os = "macos"))]
fn fd_path(fd: &std::fs::File) -> Option<PathBuf> {
    use std::os::unix::io::AsRawFd;
    let mut buf = vec![0u8; 4096];
    // SAFETY: `buf` is a valid out-pointer for F_GETPATH's
    // NUL-terminated path write.
    let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(PathBuf::from(
        String::from_utf8_lossy(&buf[..end]).into_owned(),
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn fd_path(_fd: &std::fs::File) -> Option<PathBuf> {
    None
}

/// unlinkat relative to the anchored state dir (#225 C08 eviction).
#[cfg(unix)]
fn unlinkat_state(dirfd: &std::fs::File, name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let Ok(c) = std::ffi::CString::new(name.as_bytes()) else {
        return false;
    };
    // SAFETY: `dirfd` is open; `c` is a valid relative name;
    // AT_REMOVEDIR=0 unlinks a non-directory entry only.
    unsafe { libc::unlinkat(dirfd.as_raw_fd(), c.as_ptr(), 0) == 0 }
}

/// Bound hook-state retention (#225 C08): dedup markers accumulate one
/// file per (session, dedup key) and nothing ever removed them. Once
/// per hook invocation — after the state dir is anchored — evict the
/// oldest trusted files when the directory exceeds the cap. Work is
/// bounded: at most MAX_SWEEP_SCAN entries are examined and at most
/// MAX_SWEEP_EVICT deletions per run; only regular euid-owned
/// single-link files are ever removed.
#[cfg(unix)]
fn sweep_state_dir(dirfd: &std::fs::File, fallback: &Path) {
    use std::os::unix::fs::MetadataExt;
    const MAX_STATE_FILES: usize = 4096;
    const MAX_SWEEP_SCAN: usize = 16384;
    const MAX_SWEEP_EVICT: usize = 512;
    let dir = fd_path(dirfd).unwrap_or_else(|| fallback.to_path_buf());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut scanned = 0usize;
    let mut files: Vec<(std::ffi::OsString, std::time::SystemTime)> = Vec::new();
    for ent in entries.flatten() {
        if scanned >= MAX_SWEEP_SCAN {
            break;
        }
        scanned += 1;
        // DirEntry::metadata does not traverse symlinks — a planted
        // link is skipped, never stat'ed through.
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            continue;
        }
        files.push((
            ent.file_name(),
            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        ));
    }
    if scanned <= MAX_STATE_FILES {
        return;
    }
    files.sort_by_key(|(_, m)| *m);
    let mut evicted = 0usize;
    for (name, _) in files
        .iter()
        .take(files.len().saturating_sub(MAX_STATE_FILES))
    {
        if evicted >= MAX_SWEEP_EVICT {
            break;
        }
        if unlinkat_state(dirfd, name) {
            evicted += 1;
        }
    }
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
    emit_recommendation_list(
        env,
        trigger,
        prompt_hash,
        recommended_from(recs),
        emitted,
        reason,
        recommend_ms,
    );
}

fn emit_recommendation_list(
    env: &HookEnv,
    trigger: Trigger,
    prompt_hash: Option<String>,
    recommended: Vec<RecommendedSkill>,
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
        recommended,
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
    // #225 C06: atomically claim before computing — dependency
    // resolution is infallible, so a lost claim simply records the
    // dedup skip (same contract as the prompt path).
    if env.try_claim_marker(&dedup_key) == Some(false) {
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

    *recommend_budget -= 1;
    // #225 B08: surface only DECLARED dependencies — the loaded skill's
    // resolved `requires` links via the shared resolver definition —
    // never generic recommendations mislabeled as dependencies.
    let start = Instant::now();
    let deps: Vec<RecommendedSkill> = crate::resolver::declared_dependencies(graph, skill_name)
        .into_iter()
        .map(|(name, digest)| RecommendedSkill {
            name,
            digest: Some(digest),
        })
        .collect();
    let recommend_ms = start.elapsed().as_millis() as u64;

    if deps.is_empty() {
        emit_recommendation_list(
            env,
            Trigger::SkillLoad,
            None,
            Vec::new(),
            false,
            "no_declared_dependencies",
            Some(recommend_ms),
        );
        return None;
    }
    let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
    emit_recommendation_list(
        env,
        Trigger::SkillLoad,
        None,
        deps.clone(),
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

    // Same dedup key as the v0.3.x shell hook, upgraded to the
    // collision-resistant prompt hash (#225 C05: POSIX cksum is CRC32).
    let dedup_key = format!("prompt:{phash}");
    if env.marker_exists(&dedup_key) {
        emit_recommendation(
            env,
            Trigger::PromptSubmit,
            Some(phash.clone()),
            &[],
            false,
            "dedup_skip",
            Some(0),
        );
        return None;
    }
    // #225 C04/C06: evaluate FIRST, then atomically claim the dedup
    // marker. A transient recommender failure must not permanently
    // suppress the prompt (the old touch-before-evaluate did exactly
    // that), and the O_EXCL claim closes the concurrent check-then-
    // create race where two hooks could both inject.
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
    if env.try_claim_marker(&dedup_key) == Some(false) {
        // Lost the claim race: another hook already owns this prompt's
        // dedup slot — record the skip, inject nothing.
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
                    // #225 D04: a failed permission tightening must NOT
                    // be ignored — state would then live in a
                    // permissive shared-TMPDIR dir. Disable state ops
                    // (event log still records) rather than write into
                    // an unsecured directory.
                    Ok(fd) if restrict_dir_fd(&fd) => {
                        // #225 C08: bound marker retention once per
                        // invocation — the sweep is O(bounded scan)
                        // and only removes trusted files via the
                        // anchored dirfd.
                        sweep_state_dir(&fd, &env.state_dir);
                        env.state_fd = Some(fd);
                    }
                    _ => env.state_disabled = true,
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
