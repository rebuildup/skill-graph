//! `skill-graph adapter` — enable/disable/status runtime adapters by
//! merging hook/plugin entries into each runtime's native config.
//!
//! Rust port of v0.3.x `adapters/install.sh`. Hook commands embed an
//! absolute `skill-graph` path and invoke `skill-graph hook <rt> <mode>`;
//! nothing else on the system is touched. Entries are identified by
//! markers (not absolute paths) so versioned-path updates and legacy
//! v0.3.x installs are replaced/removed cleanly.
//!
//! Env:
//!   CODEX_HOME / XDG_CONFIG_HOME   runtime config dir overrides
//!   XDG_DATA_HOME                  opencode plugin materialize dir
//!                                  (default ~/.local/share)
//!   SKILL_GRAPH_EXTRA_OVERLAY      opt-in path to an extra settings
//!                                  file (e.g. a dotfiles-managed
//!                                  settings.local.json) that enable/
//!                                  disable/status also keep free of
//!                                  stale skill-graph hooks. Unset by
//!                                  default: no path outside the
//!                                  managed config is touched.

use crate::events::Runtime;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// v0.3.x hook binary file name (`.../skill-graph-recommend-hook`).
const LEGACY_HOOK_MARKER: &str = "skill-graph-recommend-hook";
/// v0.4.x materialized plugin dir marker (`.../skill-graph/opencode-plugin`).
const PLUGIN_MARKER: &str = "skill-graph/opencode-plugin";
/// v0.3.x bundled plugin dir marker (`.../adapters/opencode/skill-graph`).
const LEGACY_PLUGIN_MARKER: &str = "adapters/opencode/skill-graph";

/// Trailing shell comment marking a hook command as installed by
/// `adapter enable`. Arbitrary `--bin` names are supported because
/// ownership is proven by this marker, not by the binary's filename.
const HOOK_OWNER_MARKER: &str = "# skill-graph-managed";

/// stdout line that tolerates EPIPE — `adapter status | head` must not
/// panic the way `println!` would on a closed pipe.
macro_rules! outln {
    ($($arg:tt)*) => {{
        let _ = crate::cli::write_stdout_line(&format!($($arg)*));
    }};
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::Project => "--project",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    Status,
}

pub struct AdapterArgs {
    pub runtime: Runtime,
    pub scope: Scope,
    pub action: Action,
    pub bin: Option<String>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn config_path(runtime: Runtime, scope: Scope) -> PathBuf {
    match (runtime, scope) {
        (Runtime::ClaudeCode, Scope::User) => home().join(".claude/settings.json"),
        (Runtime::ClaudeCode, Scope::Project) => cwd().join(".claude/settings.json"),
        (Runtime::Codex, Scope::User) => {
            let base = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".codex"));
            base.join("hooks.json")
        }
        (Runtime::Codex, Scope::Project) => cwd().join(".codex/hooks.json"),
        (Runtime::Opencode, Scope::User) => {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".config"));
            base.join("opencode/opencode.json")
        }
        (Runtime::Opencode, Scope::Project) => cwd().join("opencode.json"),
    }
}

/// Opt-in compose overlay: an extra settings file the operator asks us
/// to keep free of stale skill-graph hooks — e.g. a dotfiles-managed
/// `settings.local.json` that re-imports `~/.claude/settings.json` at
/// compose time. Returns None unless SKILL_GRAPH_EXTRA_OVERLAY names a
/// path, so the default behavior never touches files outside the
/// managed config (including the author's personal dotfiles layout).
fn extra_overlay() -> Option<PathBuf> {
    let p = std::env::var_os("SKILL_GRAPH_EXTRA_OVERLAY")?;
    if p.is_empty() {
        return None;
    }
    Some(PathBuf::from(p))
}

/// Absolute path to the skill-graph binary to embed in hook commands.
/// Order: explicit --bin, PATH lookup (yields stable shim/symlink paths),
/// then the running executable. Candidates must actually support the
/// `hook` subcommand — a stale v0.3.x binary earlier on PATH would embed
/// a command that fails on every prompt.
fn resolve_bin(override_path: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = override_path {
        let path = PathBuf::from(p);
        if !is_executable(&path) {
            return Err(format!("--bin is not executable: {p}"));
        }
        if !supports_hook_subcommand(&path) {
            return Err(format!(
                "--bin does not support `skill-graph hook` (need >= 0.4.0): {p}"
            ));
        }
        // Hooks run with an arbitrary cwd; embed an absolute path.
        // std::path::absolute does not resolve symlinks, so stable
        // shim/symlink paths (mise, npm bin) are preserved.
        return Ok(std::path::absolute(&path).unwrap_or(path));
    }
    // SKILL_GRAPH_BIN is honored here for parity with the opencode
    // plugin, which resolves the same env var at runtime: an operator
    // pointing it at a wrapper gets consistent behavior across entry
    // points instead of hooks silently embedding a different binary.
    // The plugin accepts a bare command name (spawn resolves PATH), so
    // a separator-less value is looked up on PATH rather than treated
    // as a relative path.
    if let Some(p) = std::env::var_os("SKILL_GRAPH_BIN") {
        let raw = PathBuf::from(p);
        let is_path_form = raw.components().count() > 1 || raw.is_absolute();
        let mut candidates: Vec<PathBuf> = Vec::new();
        if is_path_form {
            candidates.push(raw.clone());
        } else if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                candidates.push(dir.join(raw.file_name().unwrap_or_default()));
            }
        }
        for candidate in candidates {
            if is_executable(&candidate) && supports_hook_subcommand(&candidate) {
                return Ok(std::path::absolute(&candidate).unwrap_or(candidate));
            }
        }
        return Err(format!(
            "SKILL_GRAPH_BIN does not resolve to an executable `skill-graph hook` binary: {}",
            raw.display()
        ));
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("skill-graph");
            if is_executable(&candidate) && supports_hook_subcommand(&candidate) {
                return Ok(std::path::absolute(&candidate).unwrap_or(candidate));
            }
        }
    }
    // current_exe is this binary; `adapter` only exists on >= 0.4.0 so it
    // always supports `hook`.
    if let Ok(exe) = std::env::current_exe() {
        return Ok(exe);
    }
    Err("skill-graph binary not found on PATH".to_string())
}

/// Probe whether `path` is a skill-graph >= 0.4.0 binary by running the
/// hook entrypoint with empty stdin: the current CLI exits 0 without
/// emitting anything, while older releases fail on the unknown
/// subcommand or missing arguments.
fn supports_hook_subcommand(path: &Path) -> bool {
    let mut child = match std::process::Command::new(path)
        .args(["hook", "claude-code", "prompt"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // #163: transient resource exhaustion stays inconclusive so a
        // flaky probe cannot fail `adapter enable`. Any other spawn
        // failure means the kernel refused to exec the file (ENOEXEC —
        // foreign-arch/malformed payload, ENOENT — dead shebang
        // interpreter, EACCES): it is not hook-capable (#168).
        Err(e) => return spawn_failure_inconclusive(&e),
    };
    // Bounded wait: a FIFO or wedged binary must not hang `adapter`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(_) => return true,
        }
    }
}

/// #163 carve-out: only resource-exhaustion spawn errors are
/// inconclusive. Every other spawn failure means the binary could not
/// be exec'd and must not be treated as hook-capable (#168).
fn spawn_failure_inconclusive(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::ETXTBSY))
    }
    #[cfg(not(unix))]
    {
        matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ResourceBusy
        )
    }
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    true
                }
            })
            .unwrap_or(false)
}

/// Shell-quote a path for embedding in a hook command string.
fn sh_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// End index of the first shell word in `s`, honoring single quotes.
/// Sufficient for commands we emit via `sh_quote` plus plain paths.
fn first_shell_word(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut in_quote = false;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                in_quote = !in_quote;
                i += 1;
            }
            // A trailing unquoted backslash would skip past the end;
            // clamp so `&s[..i]` never slices out of bounds.
            b'\\' if !in_quote => i = usize::min(i + 2, b.len()),
            c if c.is_ascii_whitespace() && !in_quote => break,
            _ => i += 1,
        }
    }
    if i == 0 { None } else { Some(&s[..i]) }
}

/// Inverse of `sh_quote` for a single shell word: strips single quotes
/// and resolves the `'\''` escape sequence back to a literal `'`.
/// Char-based so non-ASCII paths round-trip correctly.
fn unquote_shell(word: &str) -> String {
    let mut out = String::new();
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    out.push(c2);
                }
            }
            '\\' => {
                if let Some(c2) = chars.next() {
                    out.push(c2);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The materialized plugin resolves `skill-graph` via `SKILL_GRAPH_BIN`
/// or PATH at spawn time. Mirror that lookup — existence + exec bit
/// only, nothing is executed — so enable/status can diagnose a missing
/// runtime binary. Hook runtimes embed a resolved path instead.
fn opencode_bin_resolvable() -> bool {
    if let Some(p) = std::env::var_os("SKILL_GRAPH_BIN") {
        let raw = PathBuf::from(&p);
        // The plugin spawns SKILL_GRAPH_BIN directly, so a bare command
        // name resolves via PATH — mirror resolve_bin's path/bare-name
        // split instead of treating every value as a literal path.
        if raw.components().count() > 1 || raw.is_absolute() {
            return is_executable(&raw);
        }
        return std::env::var_os("PATH")
            .map(|path_var| {
                std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join(&raw)))
            })
            .unwrap_or(false);
    }
    std::env::var_os("PATH")
        .map(|path_var| {
            std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join("skill-graph")))
        })
        .unwrap_or(false)
}

fn is_hook_runtime_mode(rt: &str, mode: &str) -> bool {
    matches!(rt, "claude-code" | "codex" | "opencode") && matches!(mode, "prompt" | "skill-load")
}

/// Parse a v0.4.x hook command `<bin> hook <runtime> <mode> [marker]`.
/// Returns (embedded bin path, carries_ownership_marker).
fn v040_hook_parts(command: &str) -> Option<(String, bool)> {
    let cmd = command.trim_start();
    let word = first_shell_word(cmd)?;
    let rest = cmd[word.len()..].trim_start();
    let t: Vec<&str> = rest.split_whitespace().collect();
    let marked = match t.as_slice() {
        ["hook", rt, mode] if is_hook_runtime_mode(rt, mode) => false,
        ["hook", rt, mode, "#", "skill-graph-managed"] if is_hook_runtime_mode(rt, mode) => true,
        _ => return None,
    };
    let bin = unquote_shell(word);
    if bin.is_empty() {
        None
    } else {
        Some((bin, marked))
    }
}

/// True when some whitespace-separated command token is the v0.3.x
/// `skill-graph-recommend-hook` binary (matched by file name, so a
/// foreign command merely mentioning the string is not claimed).
fn has_legacy_hook_bin(command: &str) -> bool {
    command.split_whitespace().any(|tok| {
        let tok = tok.trim_matches(|c| c == '\'' || c == '"');
        // A URL merely ending in the marker filename (e.g. a curl|sh
        // one-liner) must not be claimed for deletion.
        if tok.contains("://") {
            return false;
        }
        // Legacy hooks always embedded an absolute/relative PATH to the
        // binary — a bare `skill-graph-recommend-hook` word (e.g. in
        // `echo skill-graph-recommend-hook`) is a foreign mention, not
        // an invocation, and must not be claimed.
        if !tok.contains('/') {
            return false;
        }
        Path::new(tok).file_name().and_then(|n| n.to_str()) == Some(LEGACY_HOOK_MARKER)
    })
}

/// A hook command is ours when it carries a legacy marker, our ownership
/// comment, or the v0.4.x invocation shape with a binary literally named
/// skill-graph (covers hooks written before the marker existed, e.g. the
/// v0.4.0 release). Foreign tools sharing the tail shape — or merely
/// containing "skill-graph" in an unrelated path — are preserved.
fn is_our_hook_command(command: &str) -> bool {
    if has_legacy_hook_bin(command) {
        return true;
    }
    match v040_hook_parts(command) {
        Some((bin, marked)) => {
            marked
                || Path::new(&bin)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "skill-graph" || n == "skill-graph.exe")
        }
        None => false,
    }
}

/// True when `marker` occurs in `s` at path/URI boundaries: preceded by
/// `/` (or at the start) and followed by end or a URI/path delimiter.
/// Keeps `my-skill-graph/opencode-plugin` and `opencode-plugin-fork`
/// from being claimed as ours.
fn marker_at_boundary(s: &str, marker: &str) -> bool {
    s.match_indices(marker).any(|(i, m)| {
        let before_ok = i == 0 || s.as_bytes()[i - 1] == b'/';
        let rest = &s[i + m.len()..];
        let after_ok = rest.is_empty() || rest.starts_with(['/', '?', '#', '%']);
        before_ok && after_ok
    })
}

fn is_our_plugin(spec: &Value) -> bool {
    spec.as_str()
        .map(|s| {
            marker_at_boundary(s, PLUGIN_MARKER) || marker_at_boundary(s, LEGACY_PLUGIN_MARKER)
        })
        .unwrap_or(false)
}

fn is_legacy_plugin(spec: &Value) -> bool {
    spec.as_str()
        .map(|s| {
            marker_at_boundary(s, LEGACY_PLUGIN_MARKER) && !marker_at_boundary(s, PLUGIN_MARKER)
        })
        .unwrap_or(false)
}

/// Hook entries to install per runtime. Each entry: (event, matcher,
/// mode, statusMessage). claude-code/codex share command shape; codex adds
/// statusMessage.
fn desired_hooks(
    bin: &Path,
    runtime: Runtime,
) -> Vec<(String, Option<String>, String, Option<String>)> {
    let bin = sh_quote(&bin.to_string_lossy());
    let rt = runtime.as_str();
    let mk = |mode: &str| format!("{bin} hook {rt} {mode} {HOOK_OWNER_MARKER}");
    match runtime {
        Runtime::ClaudeCode => vec![
            ("UserPromptSubmit".into(), None, mk("prompt"), None),
            (
                "PostToolUse".into(),
                Some("Skill".into()),
                mk("skill-load"),
                None,
            ),
        ],
        Runtime::Codex => vec![
            (
                "UserPromptSubmit".into(),
                None,
                mk("prompt"),
                Some("Recommending skills".into()),
            ),
            (
                "PostToolUse".into(),
                Some("Bash".into()),
                mk("skill-load"),
                Some("Resolving skill dependencies".into()),
            ),
        ],
        Runtime::Opencode => Vec::new(),
    }
}

fn read_config(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("{}: invalid json: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Atomic write that preserves the target's permissions and writes
/// through symlinks (dotfiles-managed configs stay linked). The temp file
/// gets a unique name created with O_EXCL so a pre-planted symlink or a
/// concurrent writer cannot redirect the write.
fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    let target = if path.is_symlink() {
        std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        path.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let perms = std::fs::metadata(&target).ok().map(|m| m.permissions());
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    // Retry on create_new collisions (PID reuse, stale tmp litter): each
    // attempt gets a fresh sequence number.
    for _ in 0..8 {
        let seq = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = target.with_file_name(format!(".{name}.tmp-{}-{seq}", std::process::id()));
        // Ok(false) = tmp name already taken; retry with a fresh seq.
        let result = (|| -> Result<bool, String> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = match opts.open(&tmp) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
                Err(e) => return Err(format!("{}: {e}", tmp.display())),
            };
            std::io::Write::write_all(&mut f, text.as_bytes())
                .map_err(|e| format!("{}: {e}", tmp.display()))?;
            if let Some(p) = &perms {
                let _ = std::fs::set_permissions(&tmp, p.clone());
            }
            std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))?;
            Ok(true)
        })();
        match result {
            Ok(true) => return Ok(()),
            Ok(false) => continue,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
    Err(format!("{}: could not create temp file", target.display()))
}

/// A project-scope config lives inside an untrusted repository: a
/// symlink there could point at an unrelated user file (`.claude/
/// settings.json -> ~/.gitconfig`), and `write_text_atomic` resolves
/// symlinks before renaming — writing through would destroy the
/// target. User scope keeps write-through for dotfiles-managed configs.
/// A project symlink that stays inside the project (e.g. a shared base
/// file next to the config) is allowed.
fn check_project_symlink(path: &Path, scope: Scope) -> Result<(), String> {
    if scope != Scope::Project || !path.is_symlink() {
        return Ok(());
    }
    let target = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    if !target.starts_with(&root) {
        return Err(format!(
            "{}: project config is a symlink resolving outside the project ({}); \
             refusing to write through it — replace the link with a regular file",
            path.display(),
            target.display()
        ));
    }
    Ok(())
}

/// Write JSON without the delete-on-empty behavior of `write_config` —
/// used for files we do not own (e.g. the dotfiles overlay).
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    write_text_atomic(path, &text)
}

fn write_config(path: &Path, value: &Value) -> Result<(), String> {
    if value.as_object().map(|o| o.is_empty()).unwrap_or(false) {
        // A config reduced to {} carries no information; remove it so
        // uninstall fully restores the pre-install state. A symlinked
        // config is user-managed: write {} through to the target instead
        // of deleting the link and orphaning the hooks in the target.
        if path.is_symlink() {
            write_text_atomic(path, "{}\n")?;
        } else {
            let _ = std::fs::remove_file(path);
        }
        return Ok(());
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    write_text_atomic(path, &text)
}

fn hook_group(command: &str, status_message: Option<&str>) -> Value {
    let mut hook = Map::new();
    hook.insert("type".into(), json!("command"));
    hook.insert("command".into(), json!(command));
    if let Some(msg) = status_message {
        hook.insert("statusMessage".into(), json!(msg));
    }
    json!({"hooks": [Value::Object(hook)]})
}

/// Remove every hook object whose command matches our markers, then prune
/// emptied matcher groups and event arrays. Mutates `config` in place.
fn strip_our_hooks(config: &mut Value) {
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut any_pruned = false;
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
            continue;
        };
        let mut pruned = false;
        groups.retain_mut(|group| {
            let mut removed = false;
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = list.len();
                list.retain(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .map(|c| !is_our_hook_command(c))
                        .unwrap_or(true)
                });
                removed = list.len() != before;
            }
            if !removed {
                // Foreign group (or one we did not touch): keep its shape
                // even when its hooks array was already empty.
                return true;
            }
            pruned = true;
            any_pruned = true;
            // Prune the group only if stripping emptied it; groups that
            // never had a `hooks` array are foreign shapes and stay.
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map(|l| !l.is_empty())
                .unwrap_or(true)
        });
        // Drop the event key only when our stripping emptied it; a
        // pre-existing empty array is foreign content and stays.
        if groups.is_empty() && pruned {
            hooks.remove(&event);
        }
    }
    // Drop the "hooks" key only when our stripping emptied it — a
    // pre-existing empty `"hooks": {}` is foreign content and stays.
    if hooks.is_empty()
        && any_pruned
        && let Some(obj) = config.as_object_mut()
    {
        obj.remove("hooks");
    }
}

/// All hook command strings in a settings/hooks config, regardless of
/// matcher/event grouping.
fn hook_commands(config: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(events) = config.get("hooks").and_then(Value::as_object) {
        for groups in events.values().filter_map(Value::as_array) {
            for group in groups {
                if let Some(list) = group.get("hooks").and_then(Value::as_array) {
                    out.extend(
                        list.iter()
                            .filter_map(|h| h.get("command").and_then(Value::as_str))
                            .map(str::to_string),
                    );
                }
            }
        }
    }
    out
}

/// Count hook entries matching our markers.
fn count_hook_entries(config: &Value) -> usize {
    hook_commands(config)
        .iter()
        .filter(|c| is_our_hook_command(c))
        .count()
}

fn strip_our_plugins(config: &mut Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    if let Some(plugins) = obj.get_mut("plugins").and_then(Value::as_array_mut) {
        let before = plugins.len();
        plugins.retain(|p| !is_our_plugin(p));
        // Only drop the key when we actually removed our entries — a
        // pre-existing empty `plugins: []` is foreign content (same
        // invariant as strip_our_hooks).
        if plugins.is_empty() && plugins.len() != before {
            obj.remove("plugins");
        }
    }
}

fn count_plugin_entries(config: &Value) -> usize {
    config
        .get("plugins")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter(|s| is_our_plugin(s)).count())
        .unwrap_or(0)
}

fn count_legacy_plugin_entries(config: &Value) -> usize {
    config
        .get("plugins")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter(|s| is_legacy_plugin(s)).count())
        .unwrap_or(0)
}

fn opencode_plugin_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"));
    base.join("skill-graph/opencode-plugin")
}

/// Percent-encode a filesystem path for use in a `file://` URI.
/// Unreserved characters plus `/` pass through; everything else (including
/// `%`, `?`, `#`, spaces, non-ASCII UTF-8) is percent-encoded so the spec
/// cannot be reinterpreted as URI syntax.
fn file_url_encode(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn opencode_plugin_spec() -> String {
    format!("file://{}", file_url_encode(&opencode_plugin_dir()))
}

/// Embedded OpenCode plugin source, materialized on `adapter enable opencode`.
const OPENCODE_PLUGIN_TS: &str = include_str!("assets/opencode-plugin.ts");

fn materialize_opencode_plugin() -> Result<PathBuf, String> {
    let dir = opencode_plugin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("index.ts");
    write_text_atomic(&file, OPENCODE_PLUGIN_TS)?;
    Ok(dir)
}

pub fn run_adapter(args: &AdapterArgs) -> Result<(), String> {
    if args.bin.is_some() && !matches!(args.action, Action::Enable) {
        return Err("--bin is only valid for `adapter enable`".to_string());
    }
    if args.runtime == Runtime::Opencode && args.bin.is_some() {
        return Err(
            "--bin is not supported for opencode (the plugin resolves skill-graph via PATH)"
                .to_string(),
        );
    }
    let path = config_path(args.runtime, args.scope);
    match args.action {
        Action::Status => status(args, &path),
        Action::Disable => disable(args, &path),
        Action::Enable => enable(args, &path),
    }
}

/// Strip skill-graph hooks from the opt-in compose overlay named by
/// SKILL_GRAPH_EXTRA_OVERLAY. The overlay may be a symlink into a
/// dotfiles repo: keep the file even when it reduces to {}, and
/// tolerate an unreadable/missing overlay. Failures warn, not fail —
/// the overlay is not the file this command was asked to manage.
/// Without the env var this is a no-op: no implicit personal paths.
fn clean_extra_overlay() {
    let Some(overlay) = extra_overlay() else {
        return;
    };
    if !(overlay.exists() || overlay.is_symlink()) {
        return;
    }
    match read_config(&overlay) {
        Ok(mut config) if config.is_object() => {
            let before = config.clone();
            strip_our_hooks(&mut config);
            if config == before {
                // Nothing of ours in the overlay — do not rewrite a
                // user-managed file (serde reorders/pretty-prints it).
                return;
            }
            match write_json(&overlay, &config) {
                Ok(()) => outln!("cleaned overlay: {}", overlay.display()),
                Err(e) => crate::cli::write_stderr_line(&format!(
                    "warning: could not clean overlay {}: {e}",
                    overlay.display()
                )),
            }
        }
        Ok(_) => crate::cli::write_stderr_line(&format!(
            "warning: overlay {} is not a JSON object; left untouched",
            overlay.display()
        )),
        Err(e) => crate::cli::write_stderr_line(&format!(
            "warning: could not read overlay {}: {e}",
            overlay.display()
        )),
    }
}

fn enable(args: &AdapterArgs, path: &Path) -> Result<(), String> {
    check_project_symlink(path, args.scope)?;
    // Resolve before touching anything: a resolution failure must not leave
    // a half-written install. OpenCode uses a file:// plugin specifier and
    // never embeds the binary path.
    let bin = if args.runtime == Runtime::Opencode {
        PathBuf::new()
    } else {
        resolve_bin(args.bin.as_deref())?
    };
    let mut config = read_config(path)?;
    if !config.is_object() {
        return Err(format!(
            "{}: top-level JSON must be an object",
            path.display()
        ));
    }

    if args.runtime == Runtime::Opencode {
        if !opencode_bin_resolvable() {
            // Warn rather than fail: the runtime's PATH may differ from
            // this process's, and the plugin fails open until a binary
            // appears. `status` reports the same condition as broken.
            crate::cli::write_stderr_line(
                "warning: no skill-graph binary resolvable via SKILL_GRAPH_BIN/PATH; \
                 the plugin will fail open until one is installed",
            );
        }
        // Drop any prior skill-graph specifier (any generation), then append ours.
        strip_our_plugins(&mut config);
        let obj = config
            .as_object_mut()
            .ok_or_else(|| format!("{}: top-level JSON must be an object", path.display()))?;
        let plugins = obj.entry("plugins").or_insert_with(|| json!([]));
        let arr = plugins
            .as_array_mut()
            .ok_or_else(|| format!("{}: \"plugins\" must be an array", path.display()))?;
        // Materialize only after the config shape is validated — same
        // resolve/validate-before-touch ordering as the hook path.
        materialize_opencode_plugin()?;
        arr.push(json!(opencode_plugin_spec()));
    } else {
        strip_our_hooks(&mut config);
        let obj = config
            .as_object_mut()
            .ok_or_else(|| format!("{}: top-level JSON must be an object", path.display()))?;
        let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
        let hooks = hooks
            .as_object_mut()
            .ok_or_else(|| format!("{}: \"hooks\" must be an object", path.display()))?;
        for (event, matcher, command, status_message) in desired_hooks(&bin, args.runtime) {
            let mut group = hook_group(&command, status_message.as_deref());
            if let Some(m) = matcher {
                group
                    .as_object_mut()
                    .unwrap()
                    .insert("matcher".into(), json!(m));
            }
            hooks
                .entry(event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| format!("{}: hook event must be an array", path.display()))?
                .push(group);
        }
    }

    write_config(path, &config)?;
    outln!(
        "enabled skill-graph ({}, {}) -> {}",
        args.runtime.as_str(),
        args.scope.flag(),
        path.display()
    );
    if args.runtime != Runtime::Opencode {
        outln!("binary: {}", bin.display());
    }
    if args.runtime == Runtime::Codex {
        outln!("note: trust the hooks via /hooks in Codex (hash-pinned; unchanged across updates)");
    }
    // On compose-overlay machines an older skill-graph entry in the
    // overlay would be re-imported alongside the new one at the next
    // compose, leaving duplicate hooks. Migrate the overlay the same
    // way disable does — only when SKILL_GRAPH_EXTRA_OVERLAY opted in.
    if args.runtime == Runtime::ClaudeCode && args.scope == Scope::User {
        clean_extra_overlay();
    }
    Ok(())
}

fn disable(args: &AdapterArgs, path: &Path) -> Result<(), String> {
    check_project_symlink(path, args.scope)?;
    if !path.exists() {
        outln!("no config file: {}", path.display());
    } else {
        let mut config = read_config(path)?;
        if !config.is_object() {
            return Err(format!(
                "{}: top-level JSON must be an object",
                path.display()
            ));
        }
        let before = config.clone();
        if args.runtime == Runtime::Opencode {
            strip_our_plugins(&mut config);
        } else {
            strip_our_hooks(&mut config);
        }
        // Skip the rewrite when nothing was stripped: re-serializing a
        // foreign-only config would churn formatting for no reason
        // (same no-op guard as clean_extra_overlay).
        if config != before {
            write_config(path, &config)?;
        }
        outln!(
            "disabled skill-graph ({}, {}) from {}",
            args.runtime.as_str(),
            args.scope.flag(),
            path.display()
        );
    }

    if args.runtime == Runtime::Opencode {
        // Remove the materialized plugin only when no remaining config
        // references it — user and project scopes share one payload dir.
        // Project configs in other directories cannot be enumerated, so
        // absence of references here is best-effort.
        if !opencode_plugin_referenced() {
            let _ = std::fs::remove_dir_all(opencode_plugin_dir());
            outln!(
                "note: checked user and current-project opencode configs only; \
                 project configs in other directories may still reference the plugin"
            );
        }
    }

    if args.runtime == Runtime::ClaudeCode && args.scope == Scope::User {
        clean_extra_overlay();
    }
    Ok(())
}

/// True when any opencode config (user or project scope) still contains a
/// skill-graph plugin specifier.
fn opencode_plugin_referenced() -> bool {
    [Scope::User, Scope::Project].iter().any(|scope| {
        let path = config_path(Runtime::Opencode, *scope);
        read_config(&path)
            .map(|c| count_plugin_entries(&c) + count_legacy_plugin_entries(&c) > 0)
            .unwrap_or(false)
    })
}

fn status(args: &AdapterArgs, path: &Path) -> Result<(), String> {
    if !path.exists() {
        outln!(
            "{}: no config file ({})",
            args.runtime.as_str(),
            path.display()
        );
        return Ok(());
    }
    let config = read_config(path)?;
    if !config.is_object() {
        return Err(format!(
            "{}: top-level JSON must be an object",
            path.display()
        ));
    }
    let (ours, legacy) = if args.runtime == Runtime::Opencode {
        (
            count_plugin_entries(&config),
            count_legacy_plugin_entries(&config),
        )
    } else {
        (count_hook_entries(&config), count_legacy_entries(&config))
    };

    // Broken detection: the referenced binary/plugin file is gone while the
    // config entry remains (e.g. package manager removed the payload).
    let mut broken = false;
    if ours > 0 {
        if args.runtime == Runtime::Opencode {
            // Only the v0.4.x materialized plugin has a verifiable payload;
            // legacy file:// entries point into removed adapter bundles.
            let v040_plugin = config
                .get("plugins")
                .and_then(Value::as_array)
                .map(|p| {
                    p.iter().any(|s| {
                        s.as_str()
                            .map(|x| marker_at_boundary(x, PLUGIN_MARKER))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            let plugin_file_missing =
                v040_plugin && !opencode_plugin_dir().join("index.ts").is_file();
            let bin_missing = v040_plugin && !opencode_bin_resolvable();
            broken = plugin_file_missing || bin_missing;
            if plugin_file_missing {
                outln!(
                    "  warning: materialized plugin missing: {}",
                    opencode_plugin_dir().join("index.ts").display()
                );
            }
            if bin_missing {
                outln!("  warning: no skill-graph binary resolvable via SKILL_GRAPH_BIN/PATH");
            }
        } else {
            // Check the embedded binary of EVERY owned hook command —
            // v0.4.x and legacy v0.3.x alike: a healthy UserPromptSubmit
            // must not mask a broken PostToolUse bin, and a config whose
            // only entries are legacy hooks must not report `enabled`
            // while every invocation is dead. Existence/executable-bit
            // checks only: config-embedded paths are untrusted input
            // (project scope reads repo files) and must never be
            // executed (#167). Version capability is likewise
            // unverified here, so `enabled` means "entries present and
            // bins exist", nothing more.
            for bin_path in owned_hook_bins(&config)
                .into_iter()
                .chain(legacy_hook_bins(&config))
            {
                let bin = Path::new(&bin_path);
                if !bin.exists() {
                    broken = true;
                    outln!("  warning: embedded binary not found: {bin_path}");
                } else if !is_executable(bin) {
                    broken = true;
                    outln!("  warning: embedded binary is not executable: {bin_path}");
                }
            }
        }
    }

    let state = match (ours > 0, broken) {
        (false, _) => "disabled",
        (true, true) => "broken",
        (true, false) => "enabled",
    };
    outln!(
        "{} ({}): {} — {} entr{} in {}",
        args.runtime.as_str(),
        args.scope.flag(),
        state,
        ours,
        if ours == 1 { "y" } else { "ies" },
        path.display()
    );
    if legacy > 0 {
        outln!("  legacy v0.3.x entries present: {legacy} (run `adapter enable` to migrate)");
    }
    // Surface pending entries in the opt-in compose overlay so `status`
    // does not report a clean state while a compose would re-add hooks.
    if args.runtime == Runtime::ClaudeCode
        && args.scope == Scope::User
        && let Some(overlay) = extra_overlay()
        && (overlay.exists() || overlay.is_symlink())
        && let Ok(overlay_config) = read_config(&overlay)
    {
        let n = count_hook_entries(&overlay_config);
        if n > 0 {
            outln!(
                "  note: {n} entr{} also in overlay {}",
                if n == 1 { "y" } else { "ies" },
                overlay.display()
            );
        }
    }
    Ok(())
}

fn count_legacy_entries(config: &Value) -> usize {
    hook_commands(config)
        .iter()
        .filter(|c| has_legacy_hook_bin(c))
        .count()
}

/// Unique embedded binary paths across all owned v0.4.x hook commands
/// in `config`, in first-seen order.
fn owned_hook_bins(config: &Value) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    hook_commands(config)
        .iter()
        .filter(|c| is_our_hook_command(c))
        .filter_map(|c| embedded_bin(c))
        .filter(|b| seen.insert(b.clone()))
        .collect()
}

/// Unique legacy v0.3.x hook binary paths in `config`. `status` checks
/// these too: a config whose only entries are legacy hooks must not
/// report `enabled` while every invocation is dead.
fn legacy_hook_bins(config: &Value) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    hook_commands(config)
        .iter()
        .flat_map(|c| c.split_whitespace())
        .map(|tok| tok.trim_matches(|c| c == '\'' || c == '"'))
        .filter(|tok| {
            !tok.contains("://")
                && tok.contains('/')
                && Path::new(tok).file_name().and_then(|n| n.to_str()) == Some(LEGACY_HOOK_MARKER)
        })
        .filter(|b| seen.insert(b.to_string()))
        .map(str::to_string)
        .collect()
}

/// Embedded binary path of a v0.4.x hook command, unquoted. Only called
/// on commands already validated by `v040_hook_parts`.
fn embedded_bin(command: &str) -> Option<String> {
    v040_hook_parts(command).map(|(bin, _)| bin)
}
