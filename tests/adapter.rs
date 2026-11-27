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

    /// Like `run`, plus extra environment variables for this invocation.
    fn run_env(&self, args: &[&str], envs: &[(&str, &str)]) -> (String, i32) {
        let mut cmd = Command::new(bin());
        cmd.args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("XDG_DATA_HOME", self.home.path().join(".local/share"))
            .env("SKILL_GRAPH_BIN", self.fake_bin.to_str().unwrap())
            .env_remove("CODEX_HOME")
            .env_remove("SKILL_GRAPH_EXTRA_OVERLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// `run_full` plus extra environment variables for this invocation.
    fn run_full_env(&self, args: &[&str], envs: &[(&str, &str)]) -> (String, String, i32) {
        let mut cmd = Command::new(bin());
        cmd.args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("XDG_DATA_HOME", self.home.path().join(".local/share"))
            .env("SKILL_GRAPH_BIN", self.fake_bin.to_str().unwrap())
            .env_remove("CODEX_HOME")
            .env_remove("SKILL_GRAPH_EXTRA_OVERLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn run_full(&self, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(bin())
            .args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("XDG_DATA_HOME", self.home.path().join(".local/share"))
            .env("SKILL_GRAPH_BIN", self.fake_bin.to_str().unwrap())
            .env_remove("CODEX_HOME")
            .env_remove("SKILL_GRAPH_EXTRA_OVERLAY")
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
    // opencode resolves the binary via PATH/plugin env; --bin is rejected.
    let args: Vec<&str> = if runtime == "opencode" {
        vec!["adapter", "enable", runtime, scope_flag]
    } else {
        vec!["adapter", "enable", runtime, "--bin", &bin, scope_flag]
    };

    // enable twice (idempotent), then disable
    for _ in 0..2 {
        let (out, rc) = f.run(&args);
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
    let (out, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
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
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
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
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
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
fn foreign_tool_with_hook_tail_shape_survives() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A foreign tool whose command coincidentally ends in
    // `hook <runtime> <mode>` must not be claimed or stripped.
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/usr/bin/other-tool hook claude-code prompt"}]}]}}"#,
    )
    .unwrap();
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("other-tool hook claude-code prompt"),
        "{text}"
    );
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

#[test]
fn opencode_rejects_bin_flag() {
    let f = Fixture::new();
    let bin = f.bin_arg();
    let (_, err, rc) = f.run_full(&["adapter", "enable", "opencode", "--bin", &bin, "--user"]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("--bin"), "{err}");
}

#[test]
fn stale_bin_without_hook_is_rejected() {
    let f = Fixture::new();
    // A pre-0.4.0 style binary: executable but no `hook` subcommand.
    let stale = f.home.path().join("bin/stale-skill-graph");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (_, err, rc) = f.run_full(&[
        "adapter",
        "enable",
        "claude-code",
        "--bin",
        &stale.to_string_lossy(),
        "--user",
    ]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("does not support"), "{err}");
    assert!(!f.home.path().join(".claude/settings.json").exists());
}

#[test]
fn foreign_empty_hook_group_is_preserved() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"matcher":"X","hooks":[]},{"hooks":[{"type":"command","command":"/bin/echo hi"}]}]}}"#,
    )
    .unwrap();
    let bin = f.bin_arg();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    // The foreign group with a pre-existing empty hooks array must survive.
    assert!(text.contains(r#""matcher": "X""#), "{text}");
    assert!(text.contains("/bin/echo hi"), "{text}");
}

#[test]
fn opencode_disable_keeps_plugin_dir_while_other_scope_references_it() {
    let f = Fixture::new();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--project"]);
    assert_eq!(rc, 0);
    let plugin = f
        .home
        .path()
        .join(".local/share/skill-graph/opencode-plugin/index.ts");
    assert!(plugin.is_file());
    // Disabling project scope must not remove the payload the user
    // config still references.
    let (_, rc) = f.run(&["adapter", "disable", "opencode", "--project"]);
    assert_eq!(rc, 0);
    assert!(
        plugin.is_file(),
        "plugin dir removed while user scope active"
    );
    let (_, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    assert!(
        !plugin.exists(),
        "plugin dir should be removed after last ref"
    );
}

#[test]
fn opencode_disable_note_has_no_double_space() {
    let f = Fixture::new();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let (out, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0, "{out}");
    assert!(!out.contains("  "), "double space in output: {out:?}");
}

/// Seed a claude settings.local.json-shaped file at `rel` under the
/// fixture's fake dotfiles dir and return its path plus raw contents.
fn seed_dotfiles_overlay(f: &Fixture) -> (PathBuf, String) {
    let overlay = f
        .home
        .path()
        .join(".config/dotfiles/claude/settings.local.json");
    std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
    let seeded = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/old/bin/skill-graph hook claude-code prompt # skill-graph-managed"},{"type":"command","command":"echo keep"}]}]}}"#;
    std::fs::write(&overlay, seeded).unwrap();
    (overlay, seeded.to_string())
}

#[test]
fn default_runs_never_touch_dotfiles_overlay() {
    let f = Fixture::new();
    let (overlay, seeded) = seed_dotfiles_overlay(&f);
    let bin = f.bin_arg();
    for args in [
        vec!["adapter", "enable", "claude-code", "--bin", &bin, "--user"],
        vec!["adapter", "status", "claude-code", "--user"],
        vec!["adapter", "disable", "claude-code", "--user"],
    ] {
        let (out, rc) = f.run(&args);
        assert_eq!(rc, 0, "{args:?}: {out}");
    }
    // The personal dotfiles path must be byte-identical: no implicit
    // read-modify-write of files outside the managed config.
    assert_eq!(std::fs::read_to_string(&overlay).unwrap(), seeded);
}

#[test]
fn extra_overlay_env_opts_into_compose_cleanup() {
    let f = Fixture::new();
    let (overlay, _) = seed_dotfiles_overlay(&f);
    let overlay_str = overlay.to_string_lossy().into_owned();
    let bin = f.bin_arg();

    // Install without the env var: the overlay keeps its stale entry.
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--bin", &bin, "--user"]);
    assert_eq!(rc, 0);
    assert!(
        std::fs::read_to_string(&overlay)
            .unwrap()
            .contains("skill-graph-managed")
    );

    // status surfaces pending overlay entries when opted in
    let (out, rc) = f.run_env(
        &["adapter", "status", "claude-code", "--user"],
        &[("SKILL_GRAPH_EXTRA_OVERLAY", &overlay_str)],
    );
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("also in overlay"), "{out}");

    // re-enable migrates stale overlay entries the same way disable does
    let (out, rc) = f.run_env(
        &["adapter", "enable", "claude-code", "--bin", &bin, "--user"],
        &[("SKILL_GRAPH_EXTRA_OVERLAY", &overlay_str)],
    );
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("cleaned overlay"), "{out}");
    let text = std::fs::read_to_string(&overlay).unwrap();
    assert!(!text.contains("skill-graph-managed"), "{text}");
    assert!(text.contains("echo keep"), "{text}");
}

#[test]
fn status_broken_when_any_owned_hook_bin_missing() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let good = f.fake_bin.to_string_lossy().into_owned();
    let missing = f
        .home
        .path()
        .join("gone/bin/skill-graph")
        .to_string_lossy()
        .into_owned();

    // First hook valid, second broken -> still broken (order must not
    // matter: ANY owned hook's missing bin reports broken).
    for (first, second) in [(&good, &missing), (&missing, &good)] {
        let body = format!(
            r#"{{"hooks":{{"UserPromptSubmit":[{{"hooks":[{{"type":"command","command":"{first} hook claude-code prompt # skill-graph-managed"}}]}}],"PostToolUse":[{{"matcher":"Skill","hooks":[{{"type":"command","command":"{second} hook claude-code skill-load # skill-graph-managed"}}]}}]}}}}"#
        );
        std::fs::write(&path, &body).unwrap();
        let (out, rc) = f.run(&["adapter", "status", "claude-code", "--user"]);
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("broken"), "{out}");
        assert!(out.contains("embedded binary not found"), "{out}");
    }
}

#[test]
fn spawn_failure_is_not_hook_capable() {
    let f = Fixture::new();
    // Executable file whose shebang interpreter does not exist: the
    // kernel cannot exec it (ENOENT at spawn), so it must be rejected
    // rather than treated as an inconclusive/transient probe.
    let bad = f.home.path().join("bin/bad-shebang");
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(&bad, "#!/nonexistent/skill-graph-test-interp\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (_, err, rc) = f.run_full(&[
        "adapter",
        "enable",
        "claude-code",
        "--bin",
        &bad.to_string_lossy(),
        "--user",
    ]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("does not support"), "{err}");
    assert!(!f.home.path().join(".claude/settings.json").exists());
}

#[test]
fn trailing_backslash_command_does_not_panic() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // First shell word ending in an unquoted backslash must not panic
    // first_shell_word (byte-index-out-of-bounds on i += 2).
    for cmd in [r#""C:\\""#, r#""foo\\""#] {
        let body = format!(
            r#"{{"hooks":{{"UserPromptSubmit":[{{"hooks":[{{"type":"command","command":{cmd}}}]}}]}}}}"#
        );
        std::fs::write(&path, &body).unwrap();
        let (out, rc) = f.run(&["adapter", "status", "claude-code", "--user"]);
        assert_eq!(rc, 0, "{out}");
    }
}

#[test]
fn foreign_mention_of_legacy_marker_survives() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A bare `skill-graph-recommend-hook` word (no path separator) is a
    // foreign mention — enable/disable must not strip it.
    let body = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo skill-graph-recommend-hook"}]}]}}"#;
    std::fs::write(&path, body).unwrap();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("echo skill-graph-recommend-hook"), "{text}");
}

#[test]
fn bin_flag_rejected_for_status_and_disable() {
    let f = Fixture::new();
    let bin = f.bin_arg();
    for action in ["status", "disable"] {
        let (_, err, rc) = f.run_full(&["adapter", action, "claude-code", "--bin", &bin, "--user"]);
        assert_ne!(rc, 0, "{action}: {err}");
        assert!(err.contains("only valid for"), "{err}");
    }
}

#[test]
fn opencode_status_broken_when_bin_unresolvable() {
    let f = Fixture::new();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    // Point SKILL_GRAPH_BIN at a nonexistent path: the plugin spec is
    // present and the payload exists, but no binary resolves -> broken.
    let missing = f.home.path().join("gone/skill-graph");
    let (out, rc) = f.run_env(
        &["adapter", "status", "opencode", "--user"],
        &[("SKILL_GRAPH_BIN", missing.to_str().unwrap())],
    );
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("SKILL_GRAPH_BIN"), "{out}");
}

#[test]
fn opencode_enable_warns_when_bin_unresolvable() {
    let f = Fixture::new();
    let missing = f.home.path().join("gone/skill-graph");
    let (out, err, rc) = f.run_full_env(
        &["adapter", "enable", "opencode", "--user"],
        &[("SKILL_GRAPH_BIN", missing.to_str().unwrap())],
    );
    assert_eq!(rc, 0, "{out} {err}");
    assert!(err.contains("SKILL_GRAPH_BIN"), "{err}");
}

/// Round-9: SKILL_GRAPH_BIN may name a bare command resolved on PATH
/// (parity with the opencode plugin's runtime resolution); `adapter`
/// must embed the resolved executable, not the bare name.
#[test]
#[cfg(unix)]
fn skill_graph_bin_bare_name_resolves_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bindir = f.home.path().join("sgbin");
    std::fs::create_dir_all(&bindir).unwrap();
    let shim = bindir.join("my-sg");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_, rc) = f.run_env(
        &["adapter", "enable", "claude-code", "--user"],
        &[
            ("SKILL_GRAPH_BIN", "my-sg"),
            ("PATH", bindir.to_str().unwrap()),
        ],
    );
    assert_eq!(rc, 0);
    let cfg = f.config("claude-code", "--user").unwrap();
    let text = serde_json::to_string(&cfg).unwrap();
    assert!(
        text.contains(&shim.to_string_lossy().into_owned()),
        "embedded bin should be the PATH-resolved shim: {text}"
    );
}

/// Round-9: a SKILL_GRAPH_BIN that does not support `hook` must fail
/// enable rather than embedding a dead command.
#[test]
#[cfg(unix)]
fn skill_graph_bin_without_hook_support_errors() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bad = f.home.path().join("bin/not-sg");
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(&bad, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_, stderr, rc) = f.run_full_env(
        &["adapter", "enable", "claude-code", "--user"],
        &[("SKILL_GRAPH_BIN", bad.to_str().unwrap())],
    );
    assert_ne!(rc, 0);
    assert!(stderr.contains("SKILL_GRAPH_BIN"), "{stderr}");
}

/// Round-9: `disable` on a config with no skill-graph entries must not
/// rewrite the file — foreign formatting is preserved byte-for-byte.
#[test]
fn disable_without_our_hooks_leaves_file_bytes_untouched() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let raw = "{\n  \"hooks\": {\"UserPromptSubmit\": [{\"hooks\": [{\"type\":\"command\",\"command\":\"echo foreign\"}]}]},\n  \"model\": \"x\"\n}\n";
    std::fs::write(&path, raw).unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
}

/// Round-10: `opencode_bin_resolvable` must mirror the plugin's spawn
/// semantics — a bare command name in SKILL_GRAPH_BIN resolves via PATH,
/// not as a cwd-relative file.
#[test]
#[cfg(unix)]
fn opencode_status_accepts_bare_name_skill_graph_bin() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bindir = f.home.path().join("opbin");
    std::fs::create_dir_all(&bindir).unwrap();
    let shim = bindir.join("my-sg");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let envs = [
        ("SKILL_GRAPH_BIN", "my-sg"),
        ("PATH", bindir.to_str().unwrap()),
    ];
    // enable must not emit the "no binary resolvable" warning
    let (_, stderr, rc) = f.run_full_env(&["adapter", "enable", "opencode", "--user"], &envs);
    assert_eq!(rc, 0, "{stderr}");
    assert!(
        !stderr.contains("no skill-graph binary resolvable"),
        "{stderr}"
    );
    // and status must not report broken for the same reason
    let (out, _) = f.run_env(&["adapter", "status", "opencode", "--user"], &envs);
    assert!(out.contains("enabled"), "{out}");
    assert!(!out.contains("broken"), "{out}");
}

/// Round-10: a project-scope config symlinked outside the project must
/// not be written through — a hostile repo could point
/// `.claude/settings.json` at `~/.gitconfig` and `adapter enable` would
/// atomically replace it. Symlinks resolving inside the project stay
/// allowed (shared base-file pattern).
#[test]
#[cfg(unix)]
fn project_config_symlink_outside_project_is_refused() {
    let f = Fixture::new();
    let outside = f.home.path().join("gitconfig-target.json");
    std::fs::write(&outside, "{\"hooks\":{}}\n").unwrap();
    let link_dir = f.proj.path().join(".claude");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("settings.json");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let (_, stderr, rc) = f.run_full(&["adapter", "enable", "claude-code", "--project"]);
    assert_ne!(rc, 0, "enable should refuse: {stderr}");
    assert!(stderr.contains("outside the project"), "{stderr}");
    // The link target must be untouched.
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "{\"hooks\":{}}\n"
    );
    // The symlink itself must still be a symlink (no replace-with-file).
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

    let (_, stderr, rc) = f.run_full(&["adapter", "disable", "claude-code", "--project"]);
    assert_ne!(rc, 0, "disable should refuse: {stderr}");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "{\"hooks\":{}}\n"
    );
}

/// Round-10: `status` must existence-check legacy v0.3.x hook binaries
/// too — a config whose only entries are dead legacy hooks is `broken`,
/// not `enabled`.
#[test]
fn status_reports_broken_for_missing_legacy_bin() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"'/nonexistent/skill-graph/0.3.0/bin/skill-graph-recommend-hook' prompt"}]}]}}"#,
    )
    .unwrap();
    let (out, _) = f.run(&["adapter", "status", "claude-code", "--user"]);
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("not found"), "{out}");
}

/// A symlinked PARENT directory (`.claude -> /outside`) with a regular
/// settings.json inside must also be refused — `is_symlink()` on the
/// final component alone would miss it and write outside the project.
#[test]
fn project_config_parent_symlink_outside_project_is_refused() {
    let f = Fixture::new();
    let outside = f.home.path().join("outside-claude");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("settings.json"), "{\"hooks\":{}}\n").unwrap();
    let link = f.proj.path().join(".claude");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let (_, stderr, rc) = f.run_full(&["adapter", "enable", "claude-code", "--project"]);
    assert_ne!(rc, 0, "enable should refuse: {stderr}");
    assert!(stderr.contains("outside the project"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(outside.join("settings.json")).unwrap(),
        "{\"hooks\":{}}\n"
    );
}

/// Legacy hook detection must survive quoted paths containing spaces —
/// naive whitespace splitting would hide the binary from status.
#[test]
fn status_reports_broken_for_quoted_legacy_bin_with_spaces() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"'/tmp/sg home/0.3.0/bin/skill-graph-recommend-hook' prompt"}]}]}}"#,
    )
    .unwrap();
    let (out, _) = f.run(&["adapter", "status", "claude-code", "--user"]);
    assert!(out.contains("broken"), "{out}");
    assert!(out.contains("not found"), "{out}");
}

/// #144 round-11: legacy-hook ownership is command-position, not
/// any-word — `echo /x/…/skill-graph-recommend-hook` merely mentions
/// the binary path and must survive enable/disable untouched, while a
/// real env-prefixed legacy invocation is still claimed.
#[test]
fn legacy_marker_as_argument_is_not_ours() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo migrating from /x/0.3.0/bin/skill-graph-recommend-hook"}]}]}}"#;
    std::fs::write(&path, body).unwrap();
    let (_, rc) = f.run(&["adapter", "enable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("echo migrating from /x/0.3.0/bin/skill-graph-recommend-hook"),
        "{text}"
    );
}

/// The env-assignment prefix used by real v0.3.x hooks must not hide
/// the legacy binary from ownership detection.
#[test]
fn env_prefixed_legacy_invocation_is_ours() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"SKILL_GRAPH_BIN='/x/shims/skill-graph' '/x/0.3.0/bin/skill-graph-recommend-hook' prompt"}]}]}}"#;
    std::fs::write(&path, body).unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    // The config held only our hook, so disable restores the
    // pre-install state by removing the file entirely; a surviving
    // file must not contain the marker.
    if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("skill-graph-recommend-hook"), "{text}");
    }
}

/// #144 round-11: disable must not delete foreign sibling keys when it
/// prunes a group whose hooks array our strip emptied.
#[test]
fn disable_preserves_foreign_sibling_keys() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A foreign group that happens to contain our v0.4.x hook shape plus
    // a foreign sibling key: stripping our hook empties `hooks` but the
    // group and its `note` must survive.
    let bin = f.bin_arg();
    let body = format!(
        r#"{{"hooks":{{"UserPromptSubmit":[{{"matcher":"X","note":"keep","hooks":[{{"type":"command","command":"{bin} hook claude-code prompt"}}]}}]}}}}"#
    );
    std::fs::write(&path, &body).unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"note\": \"keep\""), "{text}");
    assert!(text.contains("\"matcher\": \"X\""), "{text}");
    assert!(!text.contains("hook claude-code prompt"), "{text}");
}

/// Round-12: a quoted or escaped `NAME=` is a command word, not an env
/// assignment — `'FOO=bar' /x/.../skill-graph-recommend-hook` runs
/// `FOO=bar` as the command in a real shell, so the hook path is merely
/// an argument and must not be claimed as ours (enable/disable would
/// otherwise delete a foreign hook entry).
#[test]
fn quoted_env_assignment_is_not_ours() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for cmd in [
        "'FOO=bar' /x/0.3.0/bin/skill-graph-recommend-hook prompt",
        "FOO\\=bar /x/0.3.0/bin/skill-graph-recommend-hook prompt",
    ] {
        let body = format!(
            r#"{{"hooks":{{"UserPromptSubmit":[{{"hooks":[{{"type":"command","command":{}}}]}}]}}}}"#,
            serde_json::to_string(cmd).unwrap()
        );
        std::fs::write(&path, &body).unwrap();
        let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
        assert_eq!(rc, 0, "{cmd}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("skill-graph-recommend-hook"),
            "{cmd}: foreign hook was stripped: {text}"
        );
    }
}

/// Round-12: `%` is not a valid follower after the plugin marker — our
/// own file_url_encode percent-encodes `%`, so
/// `opencode-plugin%2F..%2Fevil` is a foreign specifier whose path
/// continues past the marker and must survive disable.
#[test]
fn percent_encoded_plugin_marker_is_foreign() {
    let f = Fixture::new();
    let path = f.home.path().join(".config/opencode/opencode.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"plugins":["file:///x/skill-graph/opencode-plugin%2F..%2Fevil","other-plugin"]}"#,
    )
    .unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let cfg = f.config("opencode", "--user").unwrap();
    let plugins = cfg["plugins"].as_array().unwrap();
    assert!(
        plugins
            .iter()
            .any(|p| p.as_str().unwrap().contains("opencode-plugin%2F")),
        "{cfg}"
    );
    assert_eq!(plugins.len(), 2, "{cfg}");
}

/// Round-12: adapter config reads must not block on FIFOs or exhaust
/// memory on huge/special files — project-scope trees are untrusted.
#[cfg(unix)]
#[test]
fn status_rejects_fifo_and_oversized_config() {
    let f = Fixture::new();
    let dir = f.proj.path().join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("settings.json");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    // Must return promptly — a blocking read would hang the test.
    let (_, err, rc) = f.run_full(&["adapter", "status", "claude-code", "--project"]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("not a regular file"), "{err}");

    // Oversized regular file: capped read, clean error, no OOM.
    std::fs::remove_file(&fifo).unwrap();
    let big = dir.join("settings.json");
    let chunk = vec![b' '; 1024 * 1024];
    {
        use std::io::Write;
        let mut w = std::fs::File::create(&big).unwrap();
        for _ in 0..5 {
            w.write_all(&chunk).unwrap();
        }
    }
    let (_, err, rc) = f.run_full(&["adapter", "status", "claude-code", "--project"]);
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("too large"), "{err}");
}

/// Round-12: env-derived paths must be absolute. A relative HOME is
/// treated as unusable and surfaces a clean error, not a bogus
/// cwd-relative config path.
#[test]
fn relative_home_is_rejected() {
    let f = Fixture::new();
    let (_, err, rc) = f.run_full_env(
        &["adapter", "status", "claude-code", "--user"],
        &[("HOME", "relative-home")],
    );
    assert_ne!(rc, 0, "{err}");
    assert!(err.contains("HOME"), "{err}");
}

/// #144 r12 CodeRabbit: `env` consumes its own `name=value` operands
/// after quote removal — `env 'FOO=bar' <legacy-bin>` is a real legacy
/// invocation and must be claimed, not mistaken for a foreign command.
#[test]
fn env_operand_legacy_invocation_is_ours() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"env 'FOO=bar' '/x/0.3.0/bin/skill-graph-recommend-hook' prompt"}]}]}}"#;
    std::fs::write(&path, body).unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("skill-graph-recommend-hook"), "{text}");
    }
}

/// #144 r12 CodeRabbit: an unreadable project-scope opencode config must
/// not collapse to "unreferenced" — disable keeps the shared plugin
/// payload rather than breaking the scope that still references it.
#[test]
fn disable_keeps_payload_when_project_config_unreadable() {
    let f = Fixture::new();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let payload = f
        .home
        .path()
        .join(".local/share/skill-graph/opencode-plugin");
    assert!(payload.exists());
    // Corrupt the PROJECT-scope config after enable: disable --user
    // succeeds on the user config, but the shared-payload reference
    // check can no longer prove the project scope is clean.
    let proj_cfg = f.proj.path().join("opencode.json");
    std::fs::write(&proj_cfg, "{ not json").unwrap();
    let (out, rc) = f.run(&["adapter", "disable", "opencode", "--user"]);
    assert_eq!(rc, 0, "{out}");
    assert!(payload.exists(), "payload must be kept: {out}");
    assert!(
        out.contains("keeping the materialized plugin payload"),
        "{out}"
    );
}

/// #144 r12 CodeRabbit: config reads are strict UTF-8 — invalid bytes
/// must error out instead of being lossy-rewritten and persisted.
#[test]
fn invalid_utf8_config_is_rejected_not_rewritten() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes =
        br#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo "#.to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(br#""}]}]}}"#);
    std::fs::write(&path, &bytes).unwrap();
    let before = std::fs::read(&path).unwrap();
    let (out, rc) = f.run(&["adapter", "enable", "claude-code", "--user"]);
    assert_ne!(rc, 0, "{out}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "config must be untouched"
    );
}

/// Round-13: hand-edited configs may double-quote the binary path or
/// hook arguments; ownership detection must still claim the command.
#[test]
fn double_quoted_v040_hook_command_is_ours() {
    let f = Fixture::new();
    let path = f.home.path().join(".claude/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"\"/x/bin/skill-graph\" hook \"claude-code\" prompt"}]}]}}"#;
    std::fs::write(&path, body).unwrap();
    let (_, rc) = f.run(&["adapter", "disable", "claude-code", "--user"]);
    assert_eq!(rc, 0);
    if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("skill-graph"), "{text}");
    }
}

/// Round-13: SKILL_GRAPH_EXTRA_OVERLAY must be absolute like the other
/// env paths — a relative value resolves against the caller's cwd and
/// must be ignored rather than rewritten.
#[test]
fn relative_extra_overlay_is_ignored() {
    let f = Fixture::new();
    f.seed("claude-code", "--user");
    let overlay = f.proj.path().join("overlay.json");
    std::fs::write(
        &overlay,
        r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/x/bin/skill-graph hook claude-code prompt"}]}]}}"#,
    )
    .unwrap();
    let (_, rc) = f.run_env(
        &["adapter", "enable", "claude-code", "--user"],
        &[("SKILL_GRAPH_EXTRA_OVERLAY", "overlay.json")],
    );
    assert_eq!(rc, 0);
    let text = std::fs::read_to_string(&overlay).unwrap();
    assert!(
        text.contains("skill-graph"),
        "relative overlay path must be ignored: {text}"
    );
}

/// Round-16: empty SKILL_GRAPH_BIN means unset for the plugin bin
/// check too — status must fall back to the PATH probe instead of
/// reporting broken while the plugin resolves `skill-graph` fine.
#[test]
fn opencode_status_empty_skill_graph_bin_uses_path() {
    let f = Fixture::new();
    let (_, rc) = f.run(&["adapter", "enable", "opencode", "--user"]);
    assert_eq!(rc, 0);
    let path = format!("{}:/usr/bin:/bin", f.fake_bin.parent().unwrap().display());
    let (out, _) = f.run_env(
        &["adapter", "status", "opencode", "--user"],
        &[("SKILL_GRAPH_BIN", ""), ("PATH", path.as_str())],
    );
    assert!(out.contains("enabled"), "{out}");
    assert!(!out.contains("broken"), "{out}");
}
