//! `skill-graph mcp` — stdio MCP server (ADR-0005).
//!
//! Read-only tool surface over the same in-memory engine the CLI uses:
//! `recommend`, `discover`, `resolve`. No writes, no event log, no dedup
//! state, no config mutation — package install or MCP registration alone
//! activates nothing; each client's config entry is the opt-in path.

use crate::discovery::{default_roots, discover, output as discovery_output};
use crate::resolver::build;
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

/// Process-internal memoization (v1.0.0 spec §6): the built graph is
/// cached per roots signature for the lifetime of this server process.
/// No persistent cache — a client restart or new roots signature rebuilds.
type GraphCache = Arc<std::sync::Mutex<Option<(String, Arc<crate::resolver::Graph>)>>>;

struct SkillGraphMcp {
    graph_cache: GraphCache,
}

impl SkillGraphMcp {
    fn new() -> Self {
        Self {
            graph_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn graph_for(&self, roots: &[PathBuf]) -> Arc<crate::resolver::Graph> {
        let key = roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\u{1f}");
        let mut guard = self.graph_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, g)) = guard.as_ref()
            && *k == key
        {
            return g.clone();
        }
        let g = Arc::new(build(discover(roots)));
        *guard = Some((key, g.clone()));
        g
    }
}

/// Roots resolution shared with hook/CLI semantics: SKILL_GRAPH_ROOTS
/// (colon-separated) wins, then explicit tool `roots`, then defaults.
fn resolve_roots(arg_roots: Option<&Value>) -> Vec<PathBuf> {
    if let Ok(spec) = std::env::var("SKILL_GRAPH_ROOTS") {
        let roots: Vec<PathBuf> = spec
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    if let Some(arr) = arg_roots.and_then(Value::as_array) {
        let roots: Vec<PathBuf> = arr
            .iter()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    default_roots().into_iter().filter(|p| p.is_dir()).collect()
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
        "description": "Skill roots to search. Default: SKILL_GRAPH_ROOTS or the standard user/project roots."
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
        let result = match request.name.as_ref() {
            "recommend" => {
                let Some(query) = arg_str(&args, "query") else {
                    return Ok(tool_error("missing required argument: query"));
                };
                let roots = resolve_roots(args.get("roots"));
                let graph = self.graph_for(&roots);
                let recs = recommend(
                    &graph,
                    query,
                    arg_str(&args, "context").unwrap_or(""),
                    arg_usize(&args, "max", 3),
                );
                tool_result(&json!(recs))
            }
            "discover" => {
                let roots = resolve_roots(args.get("roots"));
                tool_result(&json!(discovery_output(&discover(&roots))))
            }
            "resolve" => {
                let Some(target) = arg_str(&args, "target") else {
                    return Ok(tool_error("missing required argument: target"));
                };
                let roots = resolve_roots(args.get("roots"));
                let graph = self.graph_for(&roots);
                tool_result(&json!(crate::cli::resolve_target(&graph, target)))
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
