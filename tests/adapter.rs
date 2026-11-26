//! Integration tests for `skill-graph adapter` — spawn the real binary
//! with isolated HOME/XDG/cwd per case.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_skill-graph")
}

struct Fixture {
    home: tempfile::TempDir,
    proj: tempfile::TempDir,
    fake_bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let fake_bin = home.path().join("bin/skill-graph");
        std::fs::create_dir_all(fake_bin.parent().unwrap()).unwrap();
        std::fs::copy(bin(), &fake_bin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            home,
            proj,
            fake_bin,
        }
    }

    fn run(&self, args: &[&str]) -> (String, i32) {
        let (out, _, rc) = self.run_full(args);
        (out, rc)
    }

    fn run_full(&self, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(bin())
            .args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("XDG_DATA_HOME", self.home.path().join(".local/share"))
            .env_remove("CODEX_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn config(&self, runtime: &str, scope: &str) -> Option<Value> {
        let path = match (runtime, scope) {
            ("claude-code", "--user") => self.home.path().join(".claude/settings.json"),
            ("claude-code", "--project") => self.proj.path().join(".claude/settings.json"),
            ("codex", "--user") => self.home.path().join(".codex/hooks.json"),
            ("codex", "--project") => self.proj.path().join(".codex/hooks.json"),
            ("opencode", "--user") => self.home.path().join(".config/opencode/opencode.json"),
            ("opencode", "--project") => self.proj.path().join("opencode.json"),
            _ => panic!("bad case"),
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
    }

    fn seed(&self, runtime: &str, scope: &str) {
        let path = match (runtime, scope) {
            ("claude-code", "--user") => self.home.path().join(".claude/settings.json"),
            ("claude-code", "--project") => self.proj.path().join(".claude/settings.json"),
            ("codex", "--user") => self.home.path().join(".codex/hooks.json"),
            ("codex", "--project") => self.proj.path().join(".codex/hooks.json"),
            ("opencode", "--user") => self.home.path().join(".config/opencode/opencode.json"),
            ("opencode", "--project") => self.proj.path().join("opencode.json"),
            _ => panic!("bad case"),
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if runtime == "opencode" {
            std::fs::write(&path, r#"{"plugins":["other-plugin"],"theme":"dark"}"#).unwrap();
        } else {
            std::fs::write(
                &path,
                r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo keep"}]}],"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo keep2"}]}]}}"#,
            )
            .unwrap();
        }
    }

    fn bin_arg(&self) -> String {
        self.fake_bin.to_string_lossy().into()
    }
}

fn skill_graph_entries(v: &Value) -> usize {
    let mut n = 0;
    if let Some(plugins) = v.get("plugins").and_then(Value::as_array) {
        n += plugins
            .iter()
            .filter(|p| {
                p.as_str()
                    .map(|s| s.contains("skill-graph"))
                    .unwrap_or(false)
            })
            .count();
    }
    if let Some(events) = v.get("hooks").and_then(Value::as_object) {
        for groups in events.values().filter_map(Value::as_array) {
            for g in groups {
                if let Some(list) = g.get("hooks").and_then(Value::as_array) {
                    n += list
                        .iter()
                        .filter(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .map(|c| {
                                    c.contains("skill-graph-recommend-hook")
                                        || c.contains("skill-graph hook")
                                })
                                .unwrap_or(false)
                        })
                        .count();
                }
            }
        }
    }
    n
}

fn run_case(runtime: &str, scope: &str) {
    let f = Fixture::new();
    f.seed(runtime, scope);
    let before = f.config(runtime, scope).unwrap();

    let scope_flag = if scope == "--project" {
        "--project"
    } else {
        "--user"
    };
    let bin = f.bin_arg();

    // enable twice (idempotent), then disable
    for _ in 0..2 {
        let (out, rc) = f.run(&["adapter", "enable", runtime, "--bin", &bin, scope_flag]);
        assert_eq!(rc, 0, "{runtime} {scope} enable: {out}");
    }
    let installed = f.config(runtime, scope).expect("config after enable");
    let entries = skill_graph_entries(&installed);
    let expected = if runtime == "opencode" { 1 } else { 2 };
    assert_eq!(entries, expected, "{runtime} {scope}: {installed}");

    // unrelated entries must survive
    if runtime == "opencode" {
        assert!(
            installed["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "other-plugin")
        );
    } else {
        assert!(
            serde_json::to_string(&installed)
                .unwrap()
                .contains("echo keep")
        );
    }

    // disable restores the seed exactly
    let (out, rc) = f.run(&["adapter", "disable", runtime, scope_flag]);
    assert_eq!(rc, 0, "{runtime} {scope} disable: {out}");
    let after = f.config(runtime, scope);
    match after {
        Some(v) => assert_eq!(v, before, "{runtime} {scope} restore"),
        None => {
            // file was deleted only if seed was empty — seeds are non-empty,
            // so the file must exist
            panic!("{runtime} {scope}: config file missing after disable");
        }
    }
}

#[test]
fn roundtrip_all_runtimes_scopes() {
    for scope in ["--user", "--project"] {
        for rt in ["claude-code", "codex", "opencode"] {
            run_case(rt, scope);
        }
    }
}

#[test]
fn legacy_v030_entries_are_replaced() {
    let f = Fixture::new();
    // seed a v0.3.x style config
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"SKILL_GRAPH_BIN='/x/shims/skill-graph' '/x/.local/share/skill-graph/0.3.0/adapters/claude-code/skill-graph/bin/skill-graph-recommend-hook' prompt"},{"type":"command","command":"echo keep"}]}]}}"#,
    )
    .unwrap();
    let bin = f.bin_arg();
    let (out, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0, "{out}");
    let cfg = f.config("claude-code", "--user").unwrap();
    assert_eq!(skill_graph_entries(&cfg), 2);
    assert!(
        !serde_json::to_string(&cfg)
            .unwrap()
            .contains("skill-graph-recommend-hook")
    );
    assert!(serde_json::to_string(&cfg).unwrap().contains("echo keep"));

    // status reports enabled, no legacy
    let (out, _) = f.run(&["adapter", "status", "claude-code", "--user"]);
    assert!(out.contains("enabled"), "{out}");
    assert!(!out.contains("legacy"), "{out}");
}

#[test]
fn status_reports_disabled_and_missing_file() {
    let f = Fixture::new();
    let (out, rc) = f.run(&["adapter", "status", "codex", "--user"]);
    assert_eq!(rc, 0);
    assert!(out.contains("no config file"), "{out}");
}

#[test]
fn opencode_enable_materializes_plugin() {
    let f = Fixture::new();
    let bin = f.bin_arg();
    let (out, rc) = f.run(&["adapter", "enable", "opencode", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0, "{out}");
    let plugin = f
        .home
        .path()
        .join(".local/share/skill-graph/opencode-plugin/index.ts");
    assert!(
        plugin.is_file(),
        "plugin not materialized: {}",
        plugin.display()
    );
    let content = std::fs::read_to_string(&plugin).unwrap();
    assert!(content.contains("@opencode/plugin"));

    // disable removes the plugins entry AND reports cleanly
    let (_, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    assert!(f.config("opencode", "--user").is_none());
    // the materialized plugin dir is removed too
    assert!(
        !f.home
            .path()
            .join(".local/share/skill-graph/opencode-plugin")
            .exists()
    );
}

#[test]
fn opencode_foreign_plugin_names_preserved() {
    let f = Fixture::new();
    let path = f.home.path().join(".config/opencode/opencode.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // names that contain "skill-graph" but are not ours
    std::fs::write(
        &path,
        r#"{"plugins":["file:///x/my-skill-graph-utils","npm:skill-graph-extra","other-plugin"]}"#,
    )
    .unwrap();
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    let cfg = f.config("opencode", "--user").unwrap();
    let plugins = cfg["plugins"].as_array().unwrap();
    for keep in [
        "file:///x/my-skill-graph-utils",
        "npm:skill-graph-extra",
        "other-plugin",
    ] {
        assert!(plugins.iter().any(|p| p == keep), "lost {keep}: {cfg}");
    }
    // disable strips only our entry
    let (_, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let cfg = f.config("opencode", "--user").unwrap();
    assert_eq!(cfg["plugins"].as_array().unwrap().len(), 3, "{cfg}");
}

#[test]
fn opencode_legacy_plugin_detected_by_status() {
    let f = Fixture::new();
    let path = f.home.path().join(".config/opencode/opencode.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"plugins":["file:///home/u/.local/share/skill-graph/0.3.0/adapters/opencode/skill-graph"]}"#,
    )
    .unwrap();
    let (out, _) = f.run(&["adapter", "status", "opencode", "--user"]);
    assert!(out.contains("enabled"), "{out}");
    assert!(out.contains("legacy"), "{out}");
    // enable migrates it to the materialized path
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    let cfg = f.config("opencode", "--user").unwrap();
    let spec = cfg["plugins"][0].as_str().unwrap();
    assert!(spec.contains("opencode-plugin"), "{spec}");
}

#[test]
fn malformed_config_returns_error_not_panic() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"hooks":"oops"}"#).unwrap();
    let bin = f.bin_arg();
    let (_, err, rc) = f.run_full(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("hooks"), "{err}");

    // top-level non-object also errors cleanly
    std::fs::write(&path, r#"["not","an","object"]"#).unwrap();
    let (_, err, rc) = f.run_full(&["adapter", "status", "claude-code", "--user"]);
    assert_ne!(rc, 0, "{err}");
}

#[test]
fn legacy_status_and_malformed_bin_name() {
    let f = Fixture::new();
    // seed a v0.3.x config and check status reports legacy BEFORE enable
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"SKILL_GRAPH_BIN='/x/shims/skill-graph' '/x/.local/share/skill-graph/0.3.0/adapters/claude-code/skill-graph/bin/skill-graph-recommend-hook' prompt"}]}]}}"#,
    )
    .unwrap();
    let (out, _) = f.run(&["adapter", "status", "claude-code", "--user"]);
    assert!(out.contains("legacy v0.3.x entries present: 1"), "{out}");

    // enable with a --bin that is NOT named skill-graph: enable/disable
    // must still recognize and remove our entries (invocation-shape match)
    let odd_bin = f.home.path().join("bin/sg");
    std::fs::copy(bin(), &odd_bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&odd_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let odd = odd_bin.to_string_lossy().into_owned();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &odd, "--user"]);
    assert_eq!(rc, 0);
    // second enable must be idempotent (no duplicate groups)
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &odd, "--user"]);
    assert_eq!(rc, 0);
    let count_v040 = |v: &Value| -> usize {
        v.get("hooks")
            .and_then(Value::as_object)
            .map(|ev| {
                ev.values()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter_map(|g| g.get("hooks").and_then(Value::as_array))
                    .flatten()
                    .filter(|h| {
                        h.get("command")
                            .and_then(Value::as_str)
                            .map(|c| c.contains(" hook claude-code "))
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    };
    let cfg = f.config("claude-code", "--user").unwrap();
    assert_eq!(count_v040(&cfg), 2, "{cfg}");
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    // config carried only our entries, so disable removes the file entirely
    assert!(f.config("claude-code", "--user").is_none());
}

#[test]
fn foreign_hook_group_without_hooks_array_survives_enable() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // legacy flat shape: hook objects directly in the event array (no
    // "hooks" wrapper) — must not be pruned by enable
    std::fs::write(
        &path,
        r#"{"hooks":{"PostToolUse":[{"type":"command","command":"echo foreign-flat"},{"matcher":"Skill","hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
    )
    .unwrap();
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("echo foreign-flat"), "{text}");
    assert!(text.contains("echo keep"), "{text}");
}

#[test]
fn status_reports_broken_when_bin_missing() {
    let f = Fixture::new();
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    // remove the embedded binary -> status must report broken
    std::fs::remove_file(&f.fake_bin).unwrap();
    let (out, _) = f.run(&["adapter", "status", "claude-code", "--user"]);
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("embedded binary not found"), "{out}");
}

#[test]
fn project_scope_uses_cwd() {
    let f = Fixture::new();
    let bin = f.bin_arg();
    let (out, rc) = f.run(&["adapter", "enable", "codex", "--bin", &bin, "--project"]);
    assert_eq!(rc, 0, "{out}");
    assert!(f.proj.path().join(".codex/hooks.json").is_file());
    assert!(!f.home.path().join(".codex/hooks.json").exists());
}
