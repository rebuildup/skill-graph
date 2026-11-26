//! `skill-graph mcp` — stdio MCP server (ADR-0005).
//!
//! Read-only tool surface over the same in-memory engine the CLI uses:
//! `recommend`, `discover`, `resolve`. No writes, no event log, no dedup
//! state, no config mutation — package install or MCP registration alone
//! activates nothing; each client's config entry is the opt-in path.

use crate::discovery::{Discovery, discover, output as discovery_output, resolve_roots};
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

/// One memoized scan per roots signature: the raw discovery (for the
/// `discover` tool) and the resolved graph (for `recommend`/`resolve`).
struct CacheEntry {
    discovery: Arc<Discovery>,
    graph: Arc<Graph>,
}

type GraphCache = Arc<std::sync::Mutex<Option<(String, Arc<CacheEntry>)>>>;

struct SkillGraphMcp {
    graph_cache: GraphCache,
}

impl SkillGraphMcp {
    fn new() -> Self {
        Self {
            graph_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn entry_for(cache: &GraphCache, roots: &[PathBuf]) -> Arc<CacheEntry> {
        // JSON array encoding is injective — no separator collision.
        let key = serde_json::to_string(
            &roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default();
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, entry)) = guard.as_ref()
            && *k == key
        {
            return entry.clone();
        }
        let discovery = Arc::new(discover(roots));
        // build() consumes the discovery; clone skills/errors (Skill is
        // Clone) so both views stay available under one cache entry.
        let graph = Arc::new(build(Discovery {
            skills: discovery.skills.clone(),
            errors: discovery.errors.clone(),
        }));
        let entry = Arc::new(CacheEntry { discovery, graph });
        *guard = Some((key, entry.clone()));
        entry
    }
}

/// Roots resolution shared by every entry point: SKILL_GRAPH_ROOTS wins,
/// then explicit tool `roots` (capped at MAX_TOOL_ROOTS), then defaults.
fn tool_roots(arg_roots: Option<&Value>) -> Vec<PathBuf> {
    let explicit: Vec<PathBuf> = arg_roots
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(MAX_TOOL_ROOTS + 1)
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    if explicit.len() > MAX_TOOL_ROOTS {
        // Oversized lists fall through to env/defaults rather than erroring:
        // fail-open keeps the tool usable, and the cap still bounds work.
        return crate::discovery::resolve_roots(Vec::new(), crate::discovery::default_roots);
    }
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
        let cache = self.graph_cache.clone();
        // Discovery/recommend/resolve do synchronous filesystem scans and
        // CPU-bound scoring; run them on the blocking pool so a slow or
        // huge corpus cannot stall the stdio server's async runtime.
        tokio::task::spawn_blocking(move || compute_tool(&cache, name.as_ref(), &args))
            .await
            .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?
    }
}

fn compute_tool(
    cache: &GraphCache,
    name: &str,
    args: &JsonObject,
) -> Result<CallToolResponse, McpError> {
    let result = match name {
        "recommend" => {
            let Some(query) = arg_str(args, "query") else {
                return Ok(tool_error("missing required argument: query"));
            };
            let roots = tool_roots(args.get("roots"));
            let entry = SkillGraphMcp::entry_for(cache, &roots);
            let recs = recommend(
                &entry.graph,
                query,
                arg_str(args, "context").unwrap_or(""),
                arg_usize(args, "max", 3),
            );
            tool_result(&json!(recs))
        }
        "discover" => {
            let roots = tool_roots(args.get("roots"));
            let entry = SkillGraphMcp::entry_for(cache, &roots);
            let mut output = discovery_output(&entry.discovery);
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
            let entry = SkillGraphMcp::entry_for(cache, &roots);
            tool_result(&json!(crate::cli::resolve_target(&entry.graph, target)))
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
