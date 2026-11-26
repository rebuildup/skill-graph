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
                "protocolVersion": "2025-11-25",
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
    let names: Vec<&str> = recs
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
