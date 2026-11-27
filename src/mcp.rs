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

struct SkillGraphMcp {
    /// Serializes tool computations. spawn_blocking has no concurrency
    /// bound of its own, and MCP clients may pipeline calls: without
    /// this, N queued calls each build a full in-memory graph (up to
    /// the 512 MiB discovery bound) simultaneously (#144 r19). One
    /// permit keeps peak memory at a single graph; queued calls wait
    /// asynchronously without consuming blocking-pool threads.
    compute_permit: Arc<tokio::sync::Semaphore>,
}

impl SkillGraphMcp {
    fn new() -> Self {
        Self {
            compute_permit: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

/// Roots resolution shared by every entry point: SKILL_GRAPH_ROOTS wins,
/// then explicit tool `roots` (capped at MAX_TOOL_ROOTS), then defaults.
///
/// A present `roots` array defines the scope even when every entry is
/// invalid: entries pass through verbatim (no `is_dir` pre-filter) so
/// `discover` reports "root is not a directory" diagnostics and returns
/// an empty scan — silently dropping them would widen the query to the
/// default corpus, the same violation #177 fixed for SKILL_GRAPH_ROOTS.
fn tool_roots(arg_roots: Option<&Value>) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    let Some(value) = arg_roots else {
        return Ok((
            resolve_roots(Vec::new(), crate::discovery::default_roots),
            Vec::new(),
        ));
    };
    let Some(arr) = value.as_array() else {
        return Err("roots must be an array of strings".to_string());
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut dropped = 0usize;
    for item in arr {
        match item.as_str() {
            Some(s) if roots.len() < MAX_TOOL_ROOTS => roots.push(PathBuf::from(s)),
            _ => dropped += 1,
        }
    }
    let mut diagnostics = Vec::new();
    if dropped > 0 {
        diagnostics.push(format!(
            "roots: {dropped} entr(ies) ignored (non-string or beyond {MAX_TOOL_ROOTS})"
        ));
    }
    if roots.is_empty() {
        // A present `roots` array defines the scope even when empty or
        // entirely invalid — `resolve_roots` would treat an empty
        // explicit list as "use defaults" and silently widen the query
        // to the full corpus (the #177 violation). An empty PathBuf is
        // never a directory, so discovery diagnoses it and returns an
        // empty scan instead.
        roots.push(PathBuf::new());
    }
    Ok((
        resolve_roots(roots, crate::discovery::default_roots),
        diagnostics,
    ))
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
        // The permit serializes computations across pipelined calls —
        // see compute_permit (#144 r19).
        let permit = self
            .compute_permit
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| McpError::internal_error(format!("compute permit: {e}"), None))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            compute_tool(name.as_ref(), &args)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?
    }
}

/// One fresh in-memory scan per tool call. Cold discover+build costs
/// ~7.7 ms at 500 skills (release; see docs/evidence/
/// 2026-11-25-c03-performance-benchmark.md), far below any interactive
/// budget, and it keeps every call consistent with the filesystem —
/// no staleness window for a long-lived stdio server.
fn build_graph(roots: &[PathBuf]) -> Graph {
    let discovery = discover(roots);
    // recommend/resolve return bare result JSON, so discovery
    // diagnostics would vanish; stderr is the MCP logging channel
    // (stdout carries the protocol). Bounded like the CLI surface.
    for err in discovery.errors.iter().take(10) {
        crate::cli::write_stderr_line(&format!("skill-graph-mcp: {err}"));
    }
    build(discovery)
}

fn compute_tool(name: &str, args: &JsonObject) -> Result<CallToolResponse, McpError> {
    let result = match name {
        "recommend" => {
            let Some(query) = arg_str(args, "query") else {
                return Ok(tool_error("missing required argument: query"));
            };
            let (roots, diagnostics) =
                tool_roots(args.get("roots")).map_err(|e| McpError::invalid_params(e, None))?;
            for err in &diagnostics {
                crate::cli::write_stderr_line(&format!("skill-graph-mcp: {err}"));
            }
            let graph = build_graph(&roots);
            // `max` is operator/agent supplied; clamp so a huge value
            // cannot materialize an unbounded response (#144 r15).
            let recs = recommend(
                &graph,
                query,
                arg_str(args, "context").unwrap_or(""),
                arg_usize(args, "max", 3).min(64),
            );
            tool_result(&json!(recs))
        }
        "discover" => {
            let (roots, diagnostics) =
                tool_roots(args.get("roots")).map_err(|e| McpError::invalid_params(e, None))?;
            // discover needs only skill summaries + errors — skip
            // resolver::build entirely: add_mentions clones every
            // body_text and the backlinks pass is O(links), work this
            // tool would discard (#144 r15).
            let discovery = discover(&roots);
            for err in discovery.errors.iter().take(10) {
                crate::cli::write_stderr_line(&format!("skill-graph-mcp: {err}"));
            }
            // skills is capped; errors need the same bound — and take()
            // avoids cloning the whole discovery error vector before
            // truncation (a hostile corpus can produce ~1k entries).
            const MAX_DISCOVER_ERRORS: usize = 256;
            let mut errors: Vec<String> = diagnostics;
            // Reserve two slots for the truncation markers pushed below
            // so the final vector never exceeds the cap (#144 r20).
            let room = MAX_DISCOVER_ERRORS
                .saturating_sub(errors.len())
                .saturating_sub(2);
            errors.extend(discovery.errors.iter().take(room).cloned());
            if discovery.errors.len() > room {
                errors.push(format!("errors truncated at {MAX_DISCOVER_ERRORS}"));
            }
            // Take before mapping: Skill::summary clones name/description/
            // paths per skill, so mapping all ~10k discovered skills only
            // to truncate to 512 wastes the clones (#144 r20).
            let skills_truncated = discovery.skills.len() > MAX_DISCOVER_SKILLS;
            let mut output = DiscoveryOutput {
                skills: discovery
                    .skills
                    .iter()
                    .take(MAX_DISCOVER_SKILLS)
                    .map(Skill::summary)
                    .collect(),
                errors,
            };
            if skills_truncated {
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
            let (roots, diagnostics) =
                tool_roots(args.get("roots")).map_err(|e| McpError::invalid_params(e, None))?;
            for err in &diagnostics {
                crate::cli::write_stderr_line(&format!("skill-graph-mcp: {err}"));
            }
            let graph = build_graph(&roots);
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
