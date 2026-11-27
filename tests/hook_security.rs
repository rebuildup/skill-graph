//! Regression tests for issue #169 (third-party audit D-01/D-02): the
//! shared-TMPDIR safety contract for hook event-log rotation and every
//! state write path — see docs/specs/contracts.md.

use serde_json::{Value, json};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn spawn_hook(args: &[&str], payload: &Value, envs: &[(String, String)]) -> Child {
    let bin = env!("CARGO_BIN_EXE_skill-graph");
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn skill-graph");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    // Close stdin so the hook's read_to_string sees EOF.
    drop(child.stdin.take());
    child
}

/// Wait for the child, failing the test if it does not exit within
/// `timeout` (a planted FIFO must never hang the hook — O_NONBLOCK).
fn wait_bounded(child: &mut Child, timeout: Duration) -> std::process::Output {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            // Reap stdout via wait_with_output semantics: try_wait already
            // reaped, so read the piped stdout handle directly.
            let mut out = Vec::new();
            use std::io::Read;
            if let Some(mut s) = child.stdout.take() {
                let _ = s.read_to_end(&mut out);
            }
            return std::process::Output {
                status,
                stdout: out,
                stderr: Vec::new(),
            };
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "skill-graph hook did not exit within {timeout:?} — likely blocked on a planted FIFO"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_hook(args: &[&str], payload: &Value, envs: &[(String, String)]) -> (String, i32) {
    let mut child = spawn_hook(args, payload, envs);
    let out = wait_bounded(&mut child, Duration::from_secs(30));
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn base_envs(dir: &tempfile::TempDir, extra: &[(&str, String)]) -> Vec<(String, String)> {
    let mut envs = vec![
        (
            "SKILL_GRAPH_EVENT_LOG".to_string(),
            dir.path().join("events.jsonl").to_string_lossy().into(),
        ),
        (
            "SKILL_GRAPH_HOOK_STATE".to_string(),
            dir.path().join("state").to_string_lossy().into(),
        ),
        (
            "SKILL_GRAPH_ROOTS".to_string(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/skills").to_string(),
        ),
    ];
    for (k, v) in extra {
        envs.push((k.to_string(), v.clone()));
    }
    envs
}

fn prompt_payload() -> Value {
    json!({"session_id":"sec1","prompt":"use alpha-skill please"})
}

/// D-01: a symlink planted at SKILL_GRAPH_EVENT_LOG must never be rotated
/// (lstat/symlink_metadata refuses non-regular files) and never written
/// through (O_NOFOLLOW open fails). The link and its target stay intact.
#[cfg(unix)]
#[test]
fn symlink_event_log_is_not_rotated_or_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "precious\n").unwrap();
    let link = dir.path().join("events.jsonl");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    // base_envs already points SKILL_GRAPH_EVENT_LOG at events.jsonl,
    // which is exactly where the symlink was planted.
    let envs = base_envs(&dir, &[("SKILL_GRAPH_EVENT_LOG_MAX", "1".to_string())]);
    let args = &["hook", "claude-code", "prompt"];
    let payload = prompt_payload();
    // Two runs: both the rotation check and the append path must refuse.
    for _ in 0..2 {
        let (_, rc) = run_hook(args, &payload, &envs);
        assert_eq!(rc, 0);
    }

    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "precious\n",
        "symlink target must be untouched"
    );
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "link must remain a symlink");
    assert!(
        !dir.path().join("events.jsonl.1").exists(),
        "symlink must never be rotated to .1"
    );
}

/// D-01: a FIFO at the event-log path must not hang the hook —
/// O_NONBLOCK makes the open fail fast — and nothing may be written.
#[cfg(unix)]
#[test]
fn fifo_event_log_does_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("events.jsonl");
    let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo failed");

    let envs = base_envs(&dir, &[]);
    let mut child = spawn_hook(&["hook", "claude-code", "prompt"], &prompt_payload(), &envs);
    let out = wait_bounded(&mut child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(0));
    // Still a FIFO, still empty (no reader ever attached).
    let meta = std::fs::symlink_metadata(&fifo).unwrap();
    assert!(meta.file_type().is_fifo());
}

/// D-01: a hardlinked event log (nlink > 1) is refused — appending would
/// modify every link to the inode, so the write fails open instead.
#[test]
fn hardlinked_event_log_is_not_appended() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("events.jsonl");
    std::fs::write(&log, "original\n").unwrap();
    std::fs::hard_link(&log, dir.path().join("alias.jsonl")).unwrap();

    let envs = base_envs(&dir, &[]);
    let (_, rc) = run_hook(&["hook", "claude-code", "prompt"], &prompt_payload(), &envs);
    assert_eq!(rc, 0);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "original\n",
        "multiply-linked log must not be appended"
    );
}

/// D-01: the .recommended session-state write obeys the same contract —
/// a hardlinked state file is not truncated/rewritten.
#[test]
fn hardlinked_recommended_state_is_not_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let rec = state.join("sec1.recommended");
    std::fs::write(&rec, "alpha-skill\n").unwrap();
    std::fs::hard_link(&rec, state.join("alias.recommended")).unwrap();

    let envs = base_envs(&dir, &[]);
    let (_, rc) = run_hook(&["hook", "claude-code", "prompt"], &prompt_payload(), &envs);
    assert_eq!(rc, 0);
    assert_eq!(
        std::fs::read_to_string(&rec).unwrap(),
        "alpha-skill\n",
        "multiply-linked session state must not be rewritten"
    );
}

/// D-01: a FIFO at the .recommended state path must not hang the hook on
/// either the read or the write path.
#[cfg(unix)]
#[test]
fn fifo_recommended_state_does_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let rec = state.join("sec1.recommended");
    let c = std::ffi::CString::new(rec.to_string_lossy().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);

    let envs = base_envs(&dir, &[]);
    let mut child = spawn_hook(&["hook", "claude-code", "prompt"], &prompt_payload(), &envs);
    let out = wait_bounded(&mut child, Duration::from_secs(15));
    assert_eq!(out.status.code(), Some(0));
    assert!(
        std::fs::symlink_metadata(&rec)
            .unwrap()
            .file_type()
            .is_fifo()
    );
}

/// D-02: the plugin must have no file-append debug path left. Debug
/// diagnostics are stderr-only and gated by NOSTATE, so
/// SKILL_GRAPH_DEBUG=1 + SKILL_GRAPH_HOOK_NOSTATE=1 can never create
/// /tmp/sg-oc-plugin.log (or any debug file).
#[test]
fn opencode_plugin_debug_is_stderr_only_and_nostate_gated() {
    let src = include_str!("../src/assets/opencode-plugin.ts");
    assert!(
        !src.contains("sg-oc-plugin.log") && !src.contains("/tmp/sg"),
        "fixed /tmp debug log path must be gone"
    );
    assert!(
        !src.contains("appendFileSync") && !src.contains("writeFileSync"),
        "plugin must not append debug output to files"
    );
    let dbg_const = src
        .lines()
        .find(|l| l.trim_start().starts_with("const DBG"))
        .expect("DBG constant");
    assert!(
        dbg_const.contains("NOSTATE"),
        "debug output must be gated by NOSTATE: {dbg_const}"
    );
    let dbg_fn = src
        .lines()
        .find(|l| l.contains("const dbg"))
        .expect("dbg function");
    assert!(
        dbg_fn.contains("console.error"),
        "debug output must go to stderr: {dbg_fn}"
    );
}

/// Sanity: the ordinary path still works — a regular owned log receives
/// events and rotates normally (guards against over-refusal).
#[test]
fn regular_owned_log_still_receives_events() {
    let dir = tempfile::tempdir().unwrap();
    let envs = base_envs(&dir, &[("SKILL_GRAPH_EVENT_LOG_MAX", "1".to_string())]);
    let args = &["hook", "claude-code", "prompt"];
    let payload = prompt_payload();
    run_hook(args, &payload, &envs);
    run_hook(args, &payload, &envs);
    let log = dir.path().join("events.jsonl");
    let rotated = dir.path().join("events.jsonl.1");
    assert!(rotated.exists(), "normal rotation must still work");
    assert!(
        std::fs::metadata(&log).unwrap().len() > 0,
        "live log must still receive events"
    );
}
