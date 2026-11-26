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

use crate::events::Runtime;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// v0.3.x shell-hook script name — legacy marker.
const LEGACY_HOOK_MARKER: &str = "skill-graph-recommend-hook";
/// v0.4.x materialized plugin dir marker (`.../skill-graph/opencode-plugin`).
const PLUGIN_MARKER: &str = "skill-graph/opencode-plugin";
/// v0.3.x bundled plugin dir marker (`.../adapters/opencode/skill-graph`).
const LEGACY_PLUGIN_MARKER: &str = "adapters/opencode/skill-graph";

/// Trailing shell comment marking a hook command as installed by
/// `adapter enable`. Arbitrary `--bin` names are supported because
/// ownership is proven by this marker, not by the binary's filename.
const HOOK_OWNER_MARKER: &str = "# skill-graph-managed";

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

fn dotfiles_overlay() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("dotfiles/claude/settings.local.json")
}

/// Absolute path to the skill-graph binary to embed in hook commands.
/// Order: explicit --bin, PATH lookup (yields stable shim/symlink paths),
/// then the running executable.
fn resolve_bin(override_path: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = override_path {
        let path = PathBuf::from(p);
        if is_executable(&path) {
            // Hooks run with an arbitrary cwd; embed an absolute path.
            // std::path::absolute does not resolve symlinks, so stable
            // shim/symlink paths (mise, npm bin) are preserved.
            return Ok(std::path::absolute(&path).unwrap_or(path));
        }
        return Err(format!("--bin is not executable: {p}"));
    }
    if let Some(path) = path_lookup("skill-graph") {
        return Ok(std::path::absolute(&path).unwrap_or(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        return Ok(exe);
    }
    Err("skill-graph binary not found on PATH".to_string())
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

fn path_lookup(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
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
            b'\\' if !in_quote => i += 2,
            c if c.is_ascii_whitespace() && !in_quote => break,
            _ => i += 1,
        }
    }
    if i == 0 { None } else { Some(&s[..i]) }
}

/// Inverse of `sh_quote` for a single shell word: strips single quotes
/// and resolves the `'\''` escape sequence back to a literal `'`.
fn unquote_shell(word: &str) -> String {
    let b = word.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    out.push(b[i] as char);
                    i += 1;
                }
                i += 1; // closing quote
            }
            b'\\' => {
                i += 1;
                if i < b.len() {
                    out.push(b[i] as char);
                    i += 1;
                }
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
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

/// A hook command is ours when it carries a legacy marker, our ownership
/// comment, or the v0.4.x invocation shape with a binary path that names
/// skill-graph (covers hooks written before the marker existed, e.g. the
/// v0.4.0 release). Foreign tools sharing the tail shape are preserved.
fn is_our_hook_command(command: &str) -> bool {
    if command.contains(LEGACY_HOOK_MARKER) {
        return true;
    }
    match v040_hook_parts(command) {
        Some((bin, marked)) => marked || bin.contains("skill-graph"),
        None => false,
    }
}

fn is_our_plugin(spec: &Value) -> bool {
    spec.as_str()
        .map(|s| s.contains(PLUGIN_MARKER) || s.contains(LEGACY_PLUGIN_MARKER))
        .unwrap_or(false)
}

fn is_legacy_plugin(spec: &Value) -> bool {
    spec.as_str()
        .map(|s| s.contains(LEGACY_PLUGIN_MARKER) && !s.contains(PLUGIN_MARKER))
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
/// through symlinks (dotfiles-managed configs stay linked).
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
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    if let Some(p) = perms {
        let _ = std::fs::set_permissions(&tmp, p);
    }
    std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))
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
        // uninstall fully restores the pre-install state.
        let _ = std::fs::remove_file(path);
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
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
            continue;
        };
        groups.retain_mut(|group| {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                list.retain(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .map(|c| !is_our_hook_command(c))
                        .unwrap_or(true)
                });
            }
            // Keep groups that never had a `hooks` array (foreign shapes);
            // only prune groups emptied by removing our entries.
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map(|l| !l.is_empty())
                .unwrap_or(true)
        });
        if groups.is_empty() {
            hooks.remove(&event);
        }
    }
    if hooks.is_empty()
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
        plugins.retain(|p| !is_our_plugin(p));
        if plugins.is_empty() {
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

fn opencode_plugin_spec() -> String {
    format!("file://{}", opencode_plugin_dir().display())
}

/// Embedded OpenCode plugin source, materialized on `adapter enable opencode`.
const OPENCODE_PLUGIN_TS: &str = include_str!("assets/opencode-plugin.ts");

fn materialize_opencode_plugin() -> Result<PathBuf, String> {
    let dir = opencode_plugin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("index.ts");
    std::fs::write(&file, OPENCODE_PLUGIN_TS).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(dir)
}

pub fn run_adapter(args: &AdapterArgs) -> Result<(), String> {
    let path = config_path(args.runtime, args.scope);
    match args.action {
        Action::Status => status(args, &path),
        Action::Disable => disable(args, &path),
        Action::Enable => enable(args, &path),
    }
}

fn enable(args: &AdapterArgs, path: &Path) -> Result<(), String> {
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
        materialize_opencode_plugin()?;
        // Drop any prior skill-graph specifier (any generation), then append ours.
        strip_our_plugins(&mut config);
        let obj = config
            .as_object_mut()
            .ok_or_else(|| format!("{}: top-level JSON must be an object", path.display()))?;
        let plugins = obj.entry("plugins").or_insert_with(|| json!([]));
        let arr = plugins
            .as_array_mut()
            .ok_or_else(|| format!("{}: \"plugins\" must be an array", path.display()))?;
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
    println!(
        "enabled skill-graph ({}, {}) -> {}",
        args.runtime.as_str(),
        args.scope.flag(),
        path.display()
    );
    if args.runtime != Runtime::Opencode {
        println!("binary: {}", bin.display());
    }
    if args.runtime == Runtime::Codex {
        println!(
            "note: trust the hooks via /hooks in Codex (hash-pinned; unchanged across updates)"
        );
    }
    Ok(())
}

fn disable(args: &AdapterArgs, path: &Path) -> Result<(), String> {
    if !path.exists() {
        println!("no config file: {}", path.display());
    } else {
        let mut config = read_config(path)?;
        if !config.is_object() {
            return Err(format!(
                "{}: top-level JSON must be an object",
                path.display()
            ));
        }
        if args.runtime == Runtime::Opencode {
            strip_our_plugins(&mut config);
        } else {
            strip_our_hooks(&mut config);
        }
        write_config(path, &config)?;
        println!(
            "disabled skill-graph ({}, {}) from {}",
            args.runtime.as_str(),
            args.scope.flag(),
            path.display()
        );
    }

    if args.runtime == Runtime::Opencode {
        // Remove the materialized plugin so no stale copy is left behind.
        let _ = std::fs::remove_dir_all(opencode_plugin_dir());
    }

    // dotfiles-managed machines compose ~/.claude/settings.json from a
    // machine-local overlay; hooks imported there would be regenerated on
    // the next compose, so disable must clean it too. The overlay may be a
    // symlink into a dotfiles repo: keep the file even when it reduces to
    // {}, and tolerate an unreadable/missing overlay.
    if args.runtime == Runtime::ClaudeCode && args.scope == Scope::User {
        let overlay = dotfiles_overlay();
        if (overlay.exists() || overlay.is_symlink())
            && let Ok(mut config) = read_config(&overlay)
            && config.is_object()
        {
            strip_our_hooks(&mut config);
            if let Err(e) = write_json(&overlay, &config) {
                eprintln!(
                    "warning: could not clean dotfiles overlay {}: {e}",
                    overlay.display()
                );
            } else {
                println!("cleaned dotfiles overlay: {}", overlay.display());
            }
        }
    }
    Ok(())
}

fn status(args: &AdapterArgs, path: &Path) -> Result<(), String> {
    if !path.exists() {
        println!(
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
                            .map(|x| x.contains(PLUGIN_MARKER))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            broken = v040_plugin && !opencode_plugin_dir().join("index.ts").is_file();
            if broken {
                println!(
                    "  warning: materialized plugin missing: {}",
                    opencode_plugin_dir().join("index.ts").display()
                );
            }
        } else if let Some(cmd) = first_hook_command(&config)
            && let Some(bin_path) = embedded_bin(&cmd)
            && !Path::new(&bin_path).exists()
        {
            broken = true;
            println!("  warning: embedded binary not found: {bin_path}");
        }
    }

    let state = match (ours > 0, broken) {
        (false, _) => "disabled",
        (true, true) => "broken",
        (true, false) => "enabled",
    };
    println!(
        "{} ({}): {} — {} entr{} in {}",
        args.runtime.as_str(),
        args.scope.flag(),
        state,
        ours,
        if ours == 1 { "y" } else { "ies" },
        path.display()
    );
    if legacy > 0 {
        println!("  legacy v0.3.x entries present: {legacy} (run `adapter enable` to migrate)");
    }
    Ok(())
}

fn count_legacy_entries(config: &Value) -> usize {
    hook_commands(config)
        .iter()
        .filter(|c| c.contains(LEGACY_HOOK_MARKER))
        .count()
}

fn first_hook_command(config: &Value) -> Option<String> {
    hook_commands(config)
        .into_iter()
        .find(|c| is_our_hook_command(c) && v040_hook_parts(c).is_some())
}

/// Embedded binary path of a v0.4.x hook command, unquoted. Only called
/// on commands already validated by `v040_hook_parts`.
fn embedded_bin(command: &str) -> Option<String> {
    v040_hook_parts(command).map(|(bin, _)| bin)
}
