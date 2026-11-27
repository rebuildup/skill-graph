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

/// HOME is only usable when set to an absolute path: an empty or
/// relative value would silently turn "user"-scope writes into
/// cwd-relative paths inside whatever project happens to be checked out.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Absolute directory override from the environment, or None when the
/// var is unset, empty, or relative — XDG-style vars treat non-absolute
/// values as invalid and fall back to the default.
fn env_absolute_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn config_path(runtime: Runtime, scope: Scope) -> Result<PathBuf, String> {
    let user_home = || -> Result<PathBuf, String> {
        home().ok_or_else(|| {
            "HOME is not set to an absolute path; cannot resolve user-scope config".to_string()
        })
    };
    Ok(match (runtime, scope) {
        (Runtime::ClaudeCode, Scope::User) => user_home()?.join(".claude/settings.json"),
        (Runtime::ClaudeCode, Scope::Project) => cwd().join(".claude/settings.json"),
        (Runtime::Codex, Scope::User) => match env_absolute_dir("CODEX_HOME") {
            Some(base) => base.join("hooks.json"),
            None => user_home()?.join(".codex/hooks.json"),
        },
        (Runtime::Codex, Scope::Project) => cwd().join(".codex/hooks.json"),
        (Runtime::Opencode, Scope::User) => match env_absolute_dir("XDG_CONFIG_HOME") {
            Some(base) => base.join("opencode/opencode.json"),
            None => user_home()?.join(".config/opencode/opencode.json"),
        },
        (Runtime::Opencode, Scope::Project) => cwd().join("opencode.json"),
    })
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
    // Same absolute-path policy as HOME/XDG env vars: a relative value
    // would resolve against whatever cwd `adapter` happens to run in
    // and clean_extra_overlay would rewrite that file (#144 r13).
    let path = PathBuf::from(p);
    path.is_absolute().then_some(path)
}

/// Absolute path to the skill-graph binary to embed in hook commands.
/// Order: explicit --bin, PATH lookup (yields stable shim/symlink paths),
/// then the running executable. Candidates must actually support the
/// `hook` subcommand — a stale v0.3.x binary earlier on PATH would embed
/// a command that fails on every prompt.
fn resolve_bin(override_path: Option<&str>) -> Result<PathBuf, String> {
    // The resolved path is embedded verbatim into hook command text;
    // a non-UTF-8 path would be corrupted by to_string_lossy into a
    // hook pointing at a nonexistent binary (#144 r14).
    let utf8 = |p: PathBuf| -> Result<PathBuf, String> {
        if p.to_str().is_none() {
            return Err(format!(
                "skill-graph binary path is not valid UTF-8 and cannot be embedded in hook commands: {}",
                p.display()
            ));
        }
        Ok(p)
    };
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
        return utf8(std::path::absolute(&path).unwrap_or(path));
    }
    // SKILL_GRAPH_BIN is honored here for parity with the opencode
    // plugin, which resolves the same env var at runtime: an operator
    // pointing it at a wrapper gets consistent behavior across entry
    // points instead of hooks silently embedding a different binary.
    // Empty means unset on both sides (#144 r15).
    // The plugin accepts a bare command name (spawn resolves PATH), so
    // a separator-less value is looked up on PATH rather than treated
    // as a relative path.
    if let Some(p) = std::env::var_os("SKILL_GRAPH_BIN").filter(|p| !p.is_empty()) {
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
                return utf8(std::path::absolute(&candidate).unwrap_or(candidate));
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
                return utf8(std::path::absolute(&candidate).unwrap_or(candidate));
            }
        }
    }
    // current_exe is this binary; `adapter` only exists on >= 0.4.0 so it
    // always supports `hook`.
    if let Ok(exe) = std::env::current_exe() {
        return utf8(exe);
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

/// End index of the first shell word in `s`, honoring single and
/// double quotes. Sufficient for commands we emit via `sh_quote` plus
/// hand-edited configs that quote the binary path either way.
fn first_shell_word(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    // 0 = normal, 1 = single-quoted, 2 = double-quoted.
    let mut quote = 0u8;
    while i < b.len() {
        let c = b[i];
        match quote {
            1 => {
                if c == b'\'' {
                    quote = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'"' {
                    quote = 0;
                    i += 1;
                } else if c == b'\\' {
                    // Inside double quotes `\"` does not end the word.
                    i = usize::min(i + 2, b.len());
                } else {
                    i += 1;
                }
            }
            _ => match c {
                b'\'' => {
                    quote = 1;
                    i += 1;
                }
                b'"' => {
                    quote = 2;
                    i += 1;
                }
                // A trailing unquoted backslash would skip past the end;
                // clamp so `&s[..i]` never slices out of bounds.
                b'\\' => i = usize::min(i + 2, b.len()),
                c if c.is_ascii_whitespace() => break,
                _ => i += 1,
            },
        }
    }
    if i == 0 { None } else { Some(&s[..i]) }
}

/// Inverse of `sh_quote` for a single shell word: strips single and
/// double quotes and resolves backslash escapes. Char-based so
/// non-ASCII paths round-trip correctly.
fn unquote_shell(word: &str) -> String {
    let mut out = String::new();
    let mut chars = word.chars().peekable();
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
            '"' => {
                while let Some(c2) = chars.next() {
                    match c2 {
                        '"' => break,
                        // Inside double quotes backslash escapes only
                        // $ ` " \ and newline; elsewhere it is literal.
                        '\\' => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                out.push(n);
                                chars.next();
                            }
                            _ => out.push('\\'),
                        },
                        _ => out.push(c2),
                    }
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
    // Quote-aware split: a hand-edited `hook "claude-code" prompt` must
    // parse the same as the unquoted form we emit.
    let words = shell_words(rest);
    let t: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
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

/// A shell word plus the metadata needed to tell a real `NAME=value`
/// environment assignment from a quoted/escaped lookalike: `'FOO=bar'`
/// and `FOO\=bar` are COMMAND words in a real shell (the hook path is
/// merely an argument), so treating them as assignments would let a
/// foreign hook be claimed as ours — or vice versa.
struct ShellWord {
    text: String,
    /// Byte index of the first unquoted, unescaped `=`, if any.
    eq_pos: Option<usize>,
    /// A quote or escape was consumed before that `=` — the name part
    /// is not a plain shell identifier, so this is not an assignment.
    dirty_before_eq: bool,
}

/// Split a shell command into words honoring single/double quotes and
/// backslash escapes, returning unquoted words. `split_whitespace`
/// shatters quoted paths containing spaces (`'/tmp/sg home/bin/…'`),
/// which would hide a legacy hook binary from ownership/status checks.
fn shell_words(command: &str) -> Vec<ShellWord> {
    let mut words = Vec::new();
    let mut cur = ShellWord {
        text: String::new(),
        eq_pos: None,
        dirty_before_eq: false,
    };
    let mut in_word = false;
    let mut chars = command.chars();
    // `dirty` records quote/escape consumption only while no real `=`
    // has been seen: quoting inside the VALUE (`FOO='b ar'`) is normal
    // and must not disqualify the assignment.
    macro_rules! push_char {
        ($c:expr, $quoted:expr) => {{
            if $quoted && cur.eq_pos.is_none() {
                cur.dirty_before_eq = true;
            }
            if !$quoted && cur.eq_pos.is_none() && $c == '=' {
                cur.eq_pos = Some(cur.text.len());
            }
            cur.text.push($c);
        }};
    }
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    push_char!(c2, true);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c2) = chars.next() {
                    if c2 == '"' {
                        break;
                    }
                    if c2 == '\\' {
                        // POSIX: inside double quotes backslash escapes
                        // only $ ` " \ (newline is a line continuation,
                        // which we do not model — same rule as
                        // `unquote_shell`, which keeps both chars).
                        match chars.next() {
                            Some(e @ ('"' | '\\' | '$' | '`')) => push_char!(e, true),
                            Some(e) => {
                                push_char!('\\', true);
                                push_char!(e, true);
                            }
                            None => push_char!('\\', true),
                        }
                    } else {
                        push_char!(c2, true);
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(e) = chars.next() {
                    // An escaped `=` is data, not an assignment separator.
                    push_char!(e, true);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::replace(
                        &mut cur,
                        ShellWord {
                            text: String::new(),
                            eq_pos: None,
                            dirty_before_eq: false,
                        },
                    ));
                    in_word = false;
                }
            }
            _ => {
                in_word = true;
                push_char!(c, false);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// A word is a `NAME=value` environment assignment when the name is a
/// valid shell identifier, `=` is not the first character, and neither
/// the name nor the `=` was quoted or escaped.
fn is_env_assignment(word: &ShellWord) -> bool {
    let Some(eq) = word.eq_pos else {
        return false;
    };
    if word.dirty_before_eq || eq == 0 {
        return false;
    }
    valid_env_name(&word.text[..eq])
}

/// `env` receives its arguments AFTER shell quote/escape removal, so
/// `env 'FOO=bar' cmd` really does set FOO — the quoting metadata that
/// disqualifies a shell-prefix assignment must not apply here. Check
/// the unquoted text instead.
fn is_env_arg(word: &ShellWord) -> bool {
    let Some(eq) = word.text.find('=') else {
        return false;
    };
    eq > 0 && valid_env_name(&word.text[..eq])
}

fn valid_env_name(name: &str) -> bool {
    name.chars()
        .enumerate()
        .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

/// Index of the shell command word: skips leading `NAME=value`
/// assignments and `env` prefixes — legacy hooks embed
/// `SKILL_GRAPH_BIN=…` before the binary, so the marker must be the
/// command word, not merely an argument (`echo /x/skill-graph-recommend-hook`
/// is a foreign mention and must not be claimed). After `env`, its
/// `name=value` arguments are recognized on the unquoted text because
/// env itself does no quote processing.
fn command_word_index(words: &[ShellWord]) -> Option<usize> {
    let mut i = 0;
    let mut after_env = false;
    while i < words.len() {
        if is_env_assignment(&words[i]) || words[i].text == "env" {
            after_env = after_env || words[i].text == "env";
            i += 1;
        } else if after_env && (is_env_arg(&words[i]) || words[i].text.starts_with('-')) {
            // env options (-i, -u NAME, --unset=NAME, ...) and their
            // name=value operands precede the command; option arguments
            // that are not assignments may be mis-skipped, which only
            // ever under-claims — the safe direction.
            i += 1;
        } else {
            break;
        }
    }
    (i < words.len()).then_some(i)
}

/// The word is a legacy v0.3.x hook binary path: contains a `/` (legacy
/// hooks always embedded a path, so a bare marker word is a foreign
/// mention) and its file name is the marker. URLs are excluded — a
/// curl|sh one-liner merely ending in the marker filename is not ours.
fn is_legacy_bin_word(tok: &str) -> bool {
    !tok.contains("://")
        && tok.contains('/')
        && Path::new(tok).file_name().and_then(|n| n.to_str()) == Some(LEGACY_HOOK_MARKER)
}

/// True when the command word is the v0.3.x
/// `skill-graph-recommend-hook` binary.
fn has_legacy_hook_bin(command: &str) -> bool {
    let words = shell_words(command);
    command_word_index(&words).is_some_and(|i| is_legacy_bin_word(&words[i].text))
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
        // Only end-of-string or a path separator may follow: our own
        // file_url_encode percent-encodes `%`, `?`, `#`, so a raw
        // delimiter after the marker means a foreign specifier whose
        // path continues past it (e.g. `opencode-plugin%2F..`).
        let after_ok = rest.is_empty() || rest.starts_with('/');
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

/// Config files live in untrusted project trees: a FIFO would block a
/// plain read forever, a device/special file could exhaust memory, and
/// a multi-GB file has no business being a settings.json. Require a
/// regular file and cap the read.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

fn read_config(path: &Path) -> Result<Value, String> {
    use std::io::Read;
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(format!("{}: not a regular file", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A file swapped for a FIFO between metadata() and open()
        // cannot block the read; the fstat below still rejects it.
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // metadata() -> open() is a TOCTOU window: re-verify the opened fd
    // is a regular file before reading.
    if !file.metadata().map(|m| m.is_file()).unwrap_or(false) {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let mut buf = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!(
            "{}: config too large (>{} bytes)",
            path.display(),
            MAX_CONFIG_BYTES
        ));
    }
    // Strict UTF-8: from_utf8_lossy would silently rewrite invalid
    // bytes to U+FFFD and persist the corruption on the next write.
    let text =
        std::str::from_utf8(&buf).map_err(|e| format!("{}: invalid utf-8: {e}", path.display()))?;
    serde_json::from_str(text).map_err(|e| format!("{}: invalid json: {e}", path.display()))
}

/// Atomic write that preserves the target's permissions and writes
/// through symlinks (dotfiles-managed configs stay linked). The temp file
/// gets a unique name created with O_EXCL so a pre-planted symlink or a
/// concurrent writer cannot redirect the write.
fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_text_atomic_mode(path, text, true)
}

/// `resolve_final_symlink` selects dotfiles semantics (write THROUGH a
/// final symlink to its target) vs managed-payload semantics (rename
/// REPLACES the symlink itself, never writing through it).
fn write_text_atomic_mode(
    path: &Path,
    text: &str,
    resolve_final_symlink: bool,
) -> Result<(), String> {
    let target = if resolve_final_symlink && path.is_symlink() {
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
    if scope != Scope::Project {
        return Ok(());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
    // `path.is_symlink()` only inspects the final component: a symlinked
    // PARENT (`proj/.claude -> /etc`) with a regular `settings.json`
    // inside would slip through and write outside the project. Walk the
    // ancestors that exist (the config or its parent dir may not exist
    // yet on first enable) and require each to canonicalize inside the
    // project root.
    for ancestor in path.ancestors() {
        if ancestor == cwd || !ancestor.starts_with(&cwd) {
            break;
        }
        if std::fs::symlink_metadata(ancestor).is_err() {
            continue;
        }
        let real =
            std::fs::canonicalize(ancestor).map_err(|e| format!("{}: {e}", ancestor.display()))?;
        if !real.starts_with(&root) {
            return Err(format!(
                "{}: project config path {} resolves outside the project ({}); \
                 refusing to write — replace the link with a real path",
                path.display(),
                ancestor.display(),
                real.display()
            ));
        }
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
    // The read side caps configs at MAX_CONFIG_BYTES; a near-limit
    // config plus added hooks could otherwise be written back oversized
    // and become unreadable to status/disable. Refuse before touching
    // the file.
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!(
            "{}: config too large after update (>{} bytes)",
            path.display(),
            MAX_CONFIG_BYTES
        ));
    }
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
            // An emptied group is dropped only when nothing foreign
            // remains: sibling keys ("note", extra matchers) are user
            // data and must survive — keep the group with its empty
            // hooks array rather than deleting them.
            match group.get("hooks").and_then(Value::as_array) {
                Some(list) if list.is_empty() => group
                    .as_object()
                    .map(|o| o.keys().any(|k| k != "hooks" && k != "matcher"))
                    .unwrap_or(false),
                _ => true,
            }
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

/// None when neither XDG_DATA_HOME nor HOME yields an absolute base —
/// callers must not fabricate a cwd-relative payload dir.
fn opencode_plugin_dir() -> Option<PathBuf> {
    let base =
        env_absolute_dir("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local/share")))?;
    Some(base.join("skill-graph/opencode-plugin"))
}

/// Percent-encode a filesystem path for use in a `file://` URI.
/// Unreserved characters plus `/` pass through; everything else (including
/// `%`, `?`, `#`, spaces, non-ASCII) is percent-encoded so the spec
/// cannot be reinterpreted as URI syntax. On Unix the raw path *bytes*
/// are encoded — `to_string_lossy` would replace non-UTF-8 components
/// with U+FFFD and emit a spec that matches nothing on disk (#144 r15).
fn file_url_encode(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    let mut out = String::new();
    for &b in &bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn opencode_plugin_spec(dir: &Path) -> String {
    format!("file://{}", file_url_encode(dir))
}

/// Embedded OpenCode plugin source, materialized on `adapter enable opencode`.
const OPENCODE_PLUGIN_TS: &str = include_str!("assets/opencode-plugin.ts");

fn materialize_opencode_plugin() -> Result<PathBuf, String> {
    let dir = opencode_plugin_dir().ok_or_else(|| {
        "cannot resolve the opencode plugin directory: XDG_DATA_HOME and HOME \
         are unset or not absolute"
            .to_string()
    })?;
    // The payload is skill-graph-managed, not user dotfiles: a symlink
    // at the payload dir or file would redirect the write into an
    // unrelated file. Replace-mode rename swaps the link itself, and
    // the pre-checks turn the common case into a clear error.
    if std::fs::symlink_metadata(&dir)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "{}: refusing to write through a symlinked plugin directory",
            dir.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("index.ts");
    write_text_atomic_mode(&file, OPENCODE_PLUGIN_TS, false)?;
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
    let path = config_path(args.runtime, args.scope)?;
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
        let plugin_dir = materialize_opencode_plugin()?;
        arr.push(json!(opencode_plugin_spec(&plugin_dir)));
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

    // Re-verify project containment immediately before the write: the
    // check at entry can be invalidated while resolve_bin probes ran,
    // and write_text_atomic resolves symlinks at write time.
    check_project_symlink(path, args.scope)?;
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
            // Same TOCTOU re-check as enable: strip_our_hooks/read_config
            // took time, and the write resolves symlinks.
            check_project_symlink(path, args.scope)?;
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
        match opencode_plugin_referenced() {
            Some(false) => {
                // An unresolvable dir means no payload could exist —
                // skip rather than fabricate a cwd-relative path.
                if let Some(dir) = opencode_plugin_dir() {
                    let _ = std::fs::remove_dir_all(dir);
                }
                outln!(
                    "note: checked user and current-project opencode configs only; \
                     project configs in other directories may still reference the plugin"
                );
            }
            Some(true) => {}
            None => {
                // A config we could not read might still reference the
                // shared payload — deleting it would break that scope.
                outln!(
                    "warning: could not verify all opencode configs; \
                     keeping the materialized plugin payload"
                );
            }
        }
    }

    if args.runtime == Runtime::ClaudeCode && args.scope == Scope::User {
        clean_extra_overlay();
    }
    Ok(())
}

/// Tri-state reference check for the shared opencode plugin payload: `Some(true)` a readable config references
/// the plugin, `Some(false)` every checkable config is clean, `None`
/// when at least one config could not be read or resolved. An
/// unreadable/oversized config must NOT collapse to "unreferenced" —
/// the caller would delete the shared payload out from under it.
fn opencode_plugin_referenced() -> Option<bool> {
    let mut referenced = false;
    let mut unknown = false;
    for scope in [Scope::User, Scope::Project] {
        let Ok(path) = config_path(Runtime::Opencode, scope) else {
            unknown = true;
            continue;
        };
        match read_config(&path) {
            Ok(c) if count_plugin_entries(&c) + count_legacy_plugin_entries(&c) > 0 => {
                referenced = true;
            }
            Ok(_) => {}
            Err(_) => unknown = true,
        }
    }
    if referenced {
        Some(true)
    } else if unknown {
        None
    } else {
        Some(false)
    }
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
            // An unresolvable payload dir means the materialized plugin
            // cannot be verified — do not claim it missing.
            let plugin_file_missing = v040_plugin
                && opencode_plugin_dir()
                    .map(|d| !d.join("index.ts").is_file())
                    .unwrap_or(false);
            let bin_missing = v040_plugin && !opencode_bin_resolvable();
            broken = plugin_file_missing || bin_missing;
            if plugin_file_missing && let Some(dir) = opencode_plugin_dir() {
                outln!(
                    "  warning: materialized plugin missing: {}",
                    dir.join("index.ts").display()
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
        .filter_map(|c| {
            let words = shell_words(c);
            command_word_index(&words).map(|i| words[i].text.clone())
        })
        .filter(|tok| is_legacy_bin_word(tok))
        .filter(|b| seen.insert(b.clone()))
        .collect()
}

/// Embedded binary path of a v0.4.x hook command, unquoted. Only called
/// on commands already validated by `v040_hook_parts`.
fn embedded_bin(command: &str) -> Option<String> {
    v040_hook_parts(command).map(|(bin, _)| bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-15: `file://` specs must percent-encode raw path bytes on
    /// Unix — `to_string_lossy` would emit U+FFFD for non-UTF-8
    /// components and produce a spec matching nothing on disk.
    #[cfg(unix)]
    #[test]
    fn file_url_encode_preserves_non_utf8_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xfffoo/skill-graph"));
        let spec = file_url_encode(path);
        assert!(spec.contains("%FF"), "spec: {spec}");
        assert!(!spec.contains("\u{FFFD}"), "spec: {spec}");
    }

    #[test]
    fn file_url_encode_escapes_uri_metacharacters() {
        let spec = file_url_encode(Path::new("/a b/100%/x?y#z"));
        assert_eq!(spec, "/a%20b/100%25/x%3Fy%23z");
    }
}
