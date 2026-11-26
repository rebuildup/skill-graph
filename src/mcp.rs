//! `skill-graph mcp` — stdio MCP server (ADR-0005).
//!
//! Read-only tool surface over the same in-memory engine the CLI uses:
//! `recommend`, `discover`, `resolve`. No writes, no event log, no dedup
//! state, no config mutation — package install or MCP registration alone
//! activates nothing; each client's config entry is the opt-in path.
//!
//! There is deliberately no graph cache: a stdio MCP server stays alive
//! for the whole agent session, so any memoization would serve stale
//! skills after a SKILL.md is added, edited, or deleted. Cold
//! discover+build is ~7.7 ms at 500 skills (release), so every tool call
//! rebuilds the graph in memory — skill edits are visible on the very
//! next call with no freshness bound to document.

use crate::discovery::{discover, resolve_roots};
use crate::model::{DiscoveryOutput, Skill};
use crate::resolver::{Graph, build};
use crate::search::recommend;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, ServiceExt};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Upper bound on roots a single tool call may request. The env var and
/// defaults are operator-controlled; this only caps agent-supplied lists.
const MAX_TOOL_ROOTS: usize = 32;

/// Cap on skills returned by `discover`; larger corpora are truncated and
/// noted in `errors` so a hostile or accidental `roots: ["/"]` cannot turn
/// the tool into an unbounded filesystem enumeration oracle.
const MAX_DISCOVER_SKILLS: usize = 512;

struct SkillGraphMcp;

impl SkillGraphMcp {
    fn new() -> Self {
        Self
    }
}

/// Roots resolution shared by every entry point: SKILL_GRAPH_ROOTS wins,
/// then explicit tool `roots` (capped at MAX_TOOL_ROOTS), then defaults.
fn tool_roots(arg_roots: Option<&Value>) -> Vec<PathBuf> {
    let explicit: Vec<PathBuf> = arg_roots
        .and_then(Value::as_array)
        .map(|arr| {
            // Scan a generous window before filtering: non-directory
            // entries must not consume the cap, and valid roots beyond
            // position MAX_TOOL_ROOTS+1 are still honored up to the cap.
            arr.iter()
                .take(8 * MAX_TOOL_ROOTS)
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .take(MAX_TOOL_ROOTS)
                .collect()
        })
        .unwrap_or_default();
    // Truncation to MAX_TOOL_ROOTS is deliberate: silently falling back
    // to env/defaults would return results for a corpus the caller did
    // not ask about.
    resolve_roots(explicit, crate::discovery::default_roots)
}

fn arg_str<'a>(args: &'a JsonObject, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_usize(args: &JsonObject, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn tool_result(value: &Value) -> CallToolResponse {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(value.clone());
    result.into()
}

fn tool_error(message: &str) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message.to_string())]).into()
}

fn schema(properties: Value, required: &[&str]) -> Arc<JsonObject> {
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    Arc::new(schema.as_object().unwrap().clone())
}

fn tools() -> Vec<Tool> {
    let roots_prop = json!({
        "type": "array",
        "items": {"type": "string"},
        "description": "Skill roots to search. SKILL_GRAPH_ROOTS (when set) overrides this; otherwise these roots are used instead of the standard user/project roots."
    });
    vec![
        Tool::new(
            "recommend",
            "Recommend Agent Skills for a task query, with reasons. Read-only.",
            schema(
                json!({
                    "query": {"type": "string", "description": "Task or question text."},
                    "context": {"type": "string", "description": "Optional working context (files, goal)."},
                    "roots": roots_prop,
                    "max": {"type": "integer", "description": "Maximum recommendations (default 3)."},
                }),
                &["query"],
            ),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false),
        ),
        Tool::new(
            "discover",
            "List discovered Agent Skills (name, path, availability) under the given roots. Read-only.",
            schema(json!({"roots": roots_prop}), &[]),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false),
        ),
        Tool::new(
            "resolve",
            "Resolve a skill by name or path to its identity (ambiguous names are reported, not guessed). Read-only.",
            schema(
                json!({
                    "target": {"type": "string", "description": "Skill name or path."},
                    "roots": roots_prop,
                }),
                &["target"],
            ),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false),
        ),
    ]
}

impl ServerHandler for SkillGraphMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "skill-graph",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Read-only Agent Skills discovery, resolution, and recommendation. \
                 Use `recommend` for task-relevant skills, `discover` to list installed \
                 skills, `resolve` to disambiguate a skill identity.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(tools())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.clone().unwrap_or_default();
        let name = request.name.clone();
        // Discovery/recommend/resolve do synchronous filesystem scans and
        // CPU-bound scoring; run them on the blocking pool so a slow or
        // huge corpus cannot stall the stdio server's async runtime.
        tokio::task::spawn_blocking(move || compute_tool(name.as_ref(), &args))
            .await
            .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?
    }
}

/// One fresh in-memory scan per tool call. Cold discover+build costs
/// ~7.7 ms at 500 skills (release; see docs/evidence/
/// 2026-11-25-c03-performance-benchmark.md), far below any interactive
/// budget, and it keeps every call consistent with the filesystem —
/// no staleness window for a long-lived stdio server.
struct BuiltGraph {
    graph: Graph,
    errors: Vec<String>,
}

fn build_graph(roots: &[PathBuf]) -> BuiltGraph {
    let discovery = discover(roots);
    let errors = discovery.errors.clone();
    BuiltGraph {
        graph: build(discovery),
        errors,
    }
}

fn compute_tool(name: &str, args: &JsonObject) -> Result<CallToolResponse, McpError> {
    let result = match name {
        "recommend" => {
            let Some(query) = arg_str(args, "query") else {
                return Ok(tool_error("missing required argument: query"));
            };
            let roots = tool_roots(args.get("roots"));
            let built = build_graph(&roots);
            let recs = recommend(
                &built.graph,
                query,
                arg_str(args, "context").unwrap_or(""),
                arg_usize(args, "max", 3),
            );
            tool_result(&json!(recs))
        }
        "discover" => {
            let roots = tool_roots(args.get("roots"));
            let built = build_graph(&roots);
            let mut output = DiscoveryOutput {
                skills: built.graph.skills.iter().map(Skill::summary).collect(),
                errors: built.errors.clone(),
            };
            if output.skills.len() > MAX_DISCOVER_SKILLS {
                output.skills.truncate(MAX_DISCOVER_SKILLS);
                output.errors.push(format!(
                    "discover output truncated to {MAX_DISCOVER_SKILLS} skills"
                ));
            }
            tool_result(&json!(output))
        }
        "resolve" => {
            let Some(target) = arg_str(args, "target") else {
                return Ok(tool_error("missing required argument: target"));
            };
            let roots = tool_roots(args.get("roots"));
            let built = build_graph(&roots);
            tool_result(&json!(crate::cli::resolve_target(&built.graph, target)))
        }
        other => {
            return Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            ));
        }
    };
    Ok(result)
}

/// Entry point for `skill-graph mcp`. Runs the stdio server until the
/// client disconnects or stdin closes.
pub fn run() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    runtime.block_on(async {
        let server = SkillGraphMcp::new()
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| format!("mcp serve: {e}"))?;
        let _ = server.waiting().await;
        Ok(())
    })
}
