//! Integration tests for `skill-graph mcp` — spawn the real binary and
//! speak JSON-RPC over stdio.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn spawn() -> Self {
        Self::spawn_with_version("2025-11-25")
    }

    fn spawn_with_version(protocol: &str) -> Self {
        let bin = env!("CARGO_BIN_EXE_skill-graph");
        let mut child = Command::new(bin)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn skill-graph mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        mcp.request(
            "initialize",
            json!({
                "protocolVersion": protocol,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }),
        );
        mcp.notify("notifications/initialized", json!({}));
        mcp
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{}", msg).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    /// Send a request and read until its response id arrives (server may
    /// interleave notifications).
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let msg: Value = serde_json::from_str(line.trim()).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture_roots() -> Value {
    json!([concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/skills")])
}

#[test]
fn initialize_and_list_tools() {
    let mut mcp = Mcp::spawn();
    let res = mcp.request("tools/list", Value::Null);
    let names: Vec<&str> = res["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["recommend", "discover", "resolve"]);
    // read-only annotations on every tool
    for t in res["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{t}");
    }
}

#[test]
fn recommend_returns_cli_shape() {
    let mut mcp = Mcp::spawn();
    let result = mcp.call(
        "recommend",
        json!({"query": "alpha task", "roots": fixture_roots(), "max": 3}),
    );
    assert_eq!(result["isError"], false, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let recs: Value = serde_json::from_str(text).unwrap();
    let names: Vec<&str> = recs["recommendations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["skill"]["name"].as_str())
        .collect();
    assert!(names.contains(&"alpha-skill"), "{names:?}");
    assert!(names.contains(&"beta-skill"), "{names:?}");
}

#[test]
fn discover_and_resolve() {
    let mut mcp = Mcp::spawn();
    let result = mcp.call("discover", json!({"roots": fixture_roots()}));
    let text = result["content"][0]["text"].as_str().unwrap();
    let disc: Value = serde_json::from_str(text).unwrap();
    assert!(disc["skills"].as_array().unwrap().len() >= 3);

    let result = mcp.call(
        "resolve",
        json!({"target": "alpha-skill", "roots": fixture_roots()}),
    );
    let text = result["content"][0]["text"].as_str().unwrap();
    let res: Value = serde_json::from_str(text).unwrap();
    assert_eq!(res["kind"], "name");
}

#[test]
fn missing_required_arg_is_tool_error_not_crash() {
    let mut mcp = Mcp::spawn();
    let result = mcp.call("recommend", json!({"roots": fixture_roots()}));
    assert_eq!(result["isError"], true, "{result}");
    // server still alive: a subsequent valid call works
    let result = mcp.call("discover", json!({"roots": fixture_roots()}));
    assert_eq!(result["isError"], false, "{result}");
}

#[test]
fn unknown_tool_is_protocol_error() {
    let mut mcp = Mcp::spawn();
    let res = mcp.request("tools/call", json!({"name": "nope", "arguments": {}}));
    assert!(res.get("error").is_some(), "{res}");
}

fn discover_skills(mcp: &mut Mcp, roots: &Value) -> Vec<Value> {
    let result = mcp.call("discover", json!({"roots": roots}));
    assert_eq!(result["isError"], false, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let disc: Value = serde_json::from_str(text).unwrap();
    disc["skills"].as_array().unwrap().clone()
}

fn skill_md(description: &str) -> String {
    format!("---\nname: fresh-skill\ndescription: {description}\n---\n\n# Fresh Skill\n")
}

/// Regression for the removed in-process graph cache: a long-lived stdio
/// server must reflect SKILL.md add/update/delete on the next call for
/// identical roots, with no restart.
#[test]
fn filesystem_changes_are_visible_without_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let roots = json!([root.to_string_lossy().to_string()]);

    let mut mcp = Mcp::spawn();

    // Baseline: the corpus is empty.
    assert!(
        !discover_skills(&mut mcp, &roots)
            .iter()
            .any(|s| s["name"] == "fresh-skill")
    );

    // Add a new SKILL.md while the server is already running.
    let skill_dir = root.join("fresh-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let v1 = "Use when verifying MCP cache freshness of skill discovery.";
    std::fs::write(skill_dir.join("SKILL.md"), skill_md(v1)).unwrap();

    let skills = discover_skills(&mut mcp, &roots);
    let fresh = skills
        .iter()
        .find(|s| s["name"] == "fresh-skill")
        .expect("new skill must appear in discover");
    assert_eq!(fresh["description"], v1);

    // Update the description; the next discover must see the new text.
    let v2 = "Use when checking that MCP servers rebuild their skill graph.";
    std::fs::write(skill_dir.join("SKILL.md"), skill_md(v2)).unwrap();
    let skills = discover_skills(&mut mcp, &roots);
    let fresh = skills
        .iter()
        .find(|s| s["name"] == "fresh-skill")
        .expect("updated skill must still appear in discover");
    assert_eq!(fresh["description"], v2);

    // resolve and recommend observe the same fresh graph.
    let result = mcp.call("resolve", json!({"target": "fresh-skill", "roots": roots}));
    let text = result["content"][0]["text"].as_str().unwrap();
    let res: Value = serde_json::from_str(text).unwrap();
    assert_eq!(res["kind"], "name", "{res}");

    let result = mcp.call(
        "recommend",
        json!({"query": "MCP servers rebuild graph", "roots": roots, "max": 3}),
    );
    assert_eq!(result["isError"], false, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let recs: Value = serde_json::from_str(text).unwrap();
    let names: Vec<&str> = recs["recommendations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["skill"]["name"].as_str())
        .collect();
    assert!(names.contains(&"fresh-skill"), "{names:?}");

    // Delete the skill; it must disappear from the very next calls.
    std::fs::remove_dir_all(&skill_dir).unwrap();
    assert!(
        !discover_skills(&mut mcp, &roots)
            .iter()
            .any(|s| s["name"] == "fresh-skill")
    );
    let result = mcp.call("resolve", json!({"target": "fresh-skill", "roots": roots}));
    let text = result["content"][0]["text"].as_str().unwrap();
    let res: Value = serde_json::from_str(text).unwrap();
    assert_eq!(res["kind"], "unresolved", "{res}");
}

/// A `roots` array that is present but entirely invalid must NOT widen
/// to the default corpus — same no-widening rule as SKILL_GRAPH_ROOTS
/// (#177). Entries pass through verbatim so `discover` reports the
/// failure as diagnostics with an empty skill list.
#[test]
fn all_invalid_explicit_roots_do_not_widen_to_defaults() {
    let mut mcp = Mcp::spawn();
    let missing = "/definitely/not/a/real/root-184-audit";
    let result = mcp.call("discover", json!({"roots": [missing]}));
    let text = result["content"][0]["text"].as_str().unwrap();
    let disc: Value = serde_json::from_str(text).unwrap();
    assert_eq!(disc["skills"].as_array().unwrap().len(), 0, "{disc}");
    assert!(
        disc["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("root")),
        "expected a root diagnostic, got {disc}"
    );

    // recommend over the same invalid scope returns no skills rather
    // than recommendations from the default corpus.
    let result = mcp.call(
        "recommend",
        json!({"query": "alpha task", "roots": [missing], "max": 3}),
    );
    let text = result["content"][0]["text"].as_str().unwrap();
    let recs: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        recs["recommendations"].as_array().unwrap().len(),
        0,
        "{recs}"
    );
}

/// #144 round-11: a present `roots` array defines the scope even when it
/// is empty or every entry is non-string — `[]` and `[123]` must NOT
/// widen to the default corpus (same rule as all-invalid string roots).
#[test]
fn empty_and_nonstring_roots_arrays_do_not_widen() {
    let mut mcp = Mcp::spawn();
    for roots in [json!([]), json!([123]), json!([null, {}])] {
        let result = mcp.call("discover", json!({"roots": roots}));
        let text = result["content"][0]["text"].as_str().unwrap();
        let disc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            disc["skills"].as_array().unwrap().len(),
            0,
            "roots={roots} widened to defaults: {disc}"
        );
    }
}

/// A non-array `roots` value is a malformed call, not a scope: it must
/// be rejected as invalid_params rather than reinterpreted.
#[test]
fn non_array_roots_is_rejected() {
    let mut mcp = Mcp::spawn();
    let res = mcp.request(
        "tools/call",
        json!({"name": "discover", "arguments": {"roots": "/tmp"}}),
    );
    assert!(res.get("error").is_some(), "expected JSON-RPC error: {res}");
}

/// #225 E08: an inbound line beyond the 4 MiB cap is dropped by the
/// transport without killing the connection — the next valid request
/// is still served. Before the cap, rmcp's 16 MiB default (or an
/// unbounded buffer on older SDKs) let a peer grow memory per line.
#[test]
fn oversized_line_is_dropped_and_server_survives() {
    let mut mcp = Mcp::spawn();
    // One 5 MiB line — over the 4 MiB inbound cap.
    let raw = "x".repeat(5 * 1024 * 1024);
    use std::io::Write as _;
    writeln!(mcp.stdin, "{raw}").unwrap();
    mcp.stdin.flush().unwrap();
    // Server must still answer a normal request afterwards.
    let res = mcp.request("tools/list", Value::Null);
    assert!(res["result"]["tools"].is_array(), "{res}");
}

/// #225 E10: `recommend` wraps its array in an object envelope so
/// structuredContent is a JSON object — valid on every protocol
/// version an initialize handshake can negotiate (2026-07-28 has no
/// initialize path in this SDK, so initialize sessions are always
/// legacy and would strip a bare array). Verified on the wire: the
/// legacy peer receives structuredContent AND the same JSON as text.
#[test]
fn structured_content_matches_negotiated_protocol() {
    let mut mcp = Mcp::spawn_with_version("2025-11-25");
    let result = mcp.call(
        "recommend",
        json!({"query": "alpha task", "roots": fixture_roots(), "max": 1}),
    );
    // Object envelope survives the legacy-peer strip rule.
    let sc = &result["structuredContent"];
    assert!(sc["recommendations"].is_array(), "{result}");
    // text content carries the identical JSON value.
    let text = result["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed, *sc, "text and structuredContent diverged");
}
