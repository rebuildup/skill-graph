use crate::discovery::{discover, output};
use crate::eval;
use crate::model::ResolveResult;
use crate::observe;
use crate::perf;
use crate::resolver::build;
use crate::search::recommend;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "skill-graph",
    version,
    about = "Agent Skills discovery and recommendation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Discover {
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long, default_value = "human")]
        format: String,
    },
    Search {
        query: String,
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long, default_value_t = 5)]
        max: usize,
        #[arg(long, default_value = "human")]
        format: String,
    },
    Recommend {
        /// Query text. Use --query-stdin to read it from stdin instead —
        /// keeps prompts beginning with `-` and prompt bodies off argv.
        query: Option<String>,
        #[arg(long)]
        query_stdin: bool,
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long, default_value_t = 3)]
        max: usize,
        #[arg(long, default_value = "human")]
        format: String,
    },
    Resolve {
        target: String,
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long, default_value = "json")]
        format: String,
    },
    Bench {
        dataset: PathBuf,
        /// Recommendation cutoff scored as the primary metric. Defaults
        /// to 3, matching the top-N the hook/plugin actually inject.
        #[arg(long, default_value_t = 3)]
        max: usize,
    },
    Perf {
        #[arg(long, value_delimiter = ',', default_value = "50,100,200,500")]
        sizes: Vec<usize>,
        #[arg(long, default_value_t = 30)]
        iterations: usize,
    },
    Observe {
        path: PathBuf,
        #[arg(long, default_value = "json")]
        format: String,
        /// Print funnel/summary metrics instead of the raw observation.
        #[arg(long)]
        summary: bool,
    },
    /// Runtime hook entry point: reads one hook payload JSON on stdin,
    /// emits Skill Usage Events, and prints additionalContext injection.
    /// Always exits 0 (fail-open).
    Hook {
        /// claude-code | codex | opencode
        runtime: String,
        /// prompt | skill-load
        mode: String,
    },
    /// stdio MCP server exposing read-only skill tools (recommend,
    /// discover, resolve). See ADR-0005.
    Mcp,
    /// Manage runtime adapter hooks/plugins in native configs.
    /// enable|disable|status <claude-code|codex|opencode> [--user|--project]
    Adapter {
        action: String,
        /// claude-code | codex | opencode
        runtime: String,
        #[arg(long)]
        user: bool,
        #[arg(long)]
        project: bool,
        /// Explicit skill-graph binary path to embed in hook commands
        #[arg(long)]
        bin: Option<String>,
    },
    /// Normalize runtime hook/plugin payloads (stdin JSON) into Skill
    /// Usage Events — one JSON object per line. Prints nothing when a
    /// payload carries no skill-load signal. Detection only — never runs
    /// recommendation or corpus analysis.
    Detect {
        #[arg(long)]
        runtime: String,
        #[arg(long, default_value = "hook")]
        via: String,
    },
}

pub fn run() -> Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Command::Discover { root, format } => {
            let roots = roots_or_default(root);
            let discovery = discover(&roots);
            print_result(output(&discovery), &format)
        }
        Command::Search {
            query,
            root,
            max,
            format,
        } => {
            let graph = graph_for(roots_or_default(root))?;
            let recs = recommend(&graph, &query, "", max);
            print_result(json!(recs), &format)
        }
        Command::Recommend {
            query,
            query_stdin,
            context,
            root,
            max,
            format,
        } => {
            let query = match (query, query_stdin) {
                (Some(q), false) => q,
                (None, true) => {
                    // Same 16 MiB bound as hook/detect stdin: query text is
                    // agent-controlled and recommend() work scales with it.
                    let mut buf = String::new();
                    std::io::stdin()
                        .take(16 * 1024 * 1024 + 1)
                        .read_to_string(&mut buf)
                        .map_err(|e| format!("failed to read --query-stdin: {e}"))?;
                    if buf.len() > 16 * 1024 * 1024 {
                        return Err("--query-stdin input exceeds 16 MiB".into());
                    }
                    buf
                }
                (Some(_), true) => {
                    return Err("pass either a query argument or --query-stdin, not both".into());
                }
                (None, false) => {
                    return Err("recommend requires a query argument or --query-stdin".into());
                }
            };
            let graph = graph_for(roots_or_default(root))?;
            let recs = recommend(&graph, &query, context.as_deref().unwrap_or(""), max);
            print_result(json!(recs), &format)
        }
        Command::Resolve {
            target,
            root,
            format,
        } => {
            let graph = graph_for(roots_or_default(root))?;
            let result = resolve_target(&graph, &target);
            print_result(json!(result), &format)
        }
        Command::Bench { dataset, max } => {
            let result = eval::run(&dataset, max)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        Command::Observe {
            path,
            format,
            summary,
        } => {
            let obs = observe::observe_path(&path)?;
            if summary {
                // Route through print_result so --format is validated
                // (bogus values error) instead of being silently ignored.
                print_result(observe::summary(&obs), &format)?;
                return Ok(());
            }
            print_result(json!(obs), &format)
        }
        Command::Hook { runtime, mode } => {
            use crate::events::Runtime;
            use std::io::Read;
            // Hooks must fail open: unusable arguments still exit 0.
            // Drain stdin first so the caller never hits EPIPE writing a
            // payload to a process that already exited.
            let parsed = Runtime::parse(&runtime);
            let valid_mode = matches!(mode.as_str(), "prompt" | "skill-load");
            match parsed {
                Some(runtime) if valid_mode => crate::hook::run_hook(runtime, &mode),
                _ => {
                    // Same 16 MiB cap as the real hook path — an
                    // invalid-args invocation must not drain unboundedly.
                    let mut sink = Vec::new();
                    let _ = std::io::Read::take(std::io::stdin(), 16 * 1024 * 1024)
                        .read_to_end(&mut sink);
                    if parsed.is_none() {
                        eprintln!("skill-graph: unknown runtime: {runtime}");
                    } else {
                        eprintln!("skill-graph: unknown hook mode: {mode}");
                    }
                    Ok(())
                }
            }
        }
        Command::Mcp => crate::mcp::run(),
        Command::Adapter {
            action,
            runtime,
            user,
            project,
            bin,
        } => {
            use crate::adapter::{AdapterArgs, Scope};
            use crate::events::Runtime;
            let runtime =
                Runtime::parse(&runtime).ok_or_else(|| format!("unknown runtime: {runtime}"))?;
            let action = match action.as_str() {
                "enable" => crate::adapter::Action::Enable,
                "disable" => crate::adapter::Action::Disable,
                "status" => crate::adapter::Action::Status,
                other => return Err(format!("unknown adapter action: {other}")),
            };
            if user && project {
                return Err("--user and --project are mutually exclusive".to_string());
            }
            crate::adapter::run_adapter(&AdapterArgs {
                runtime,
                scope: if project { Scope::Project } else { Scope::User },
                action,
                bin,
            })
        }
        Command::Detect { runtime, via } => {
            use crate::events::{self, ObservedVia, Runtime};
            // Callers pipe a payload unconditionally; drain stdin (bounded)
            // before any early error exit so the writer never hits EPIPE.
            let runtime = match Runtime::parse(&runtime) {
                Some(r) => r,
                None => return detect_arg_error(format!("unknown runtime: {runtime}")),
            };
            let via = match via.as_str() {
                "hook" => ObservedVia::Hook,
                "plugin" => ObservedVia::Plugin,
                "transcript" => ObservedVia::Transcript,
                "db" => ObservedVia::Db,
                other => {
                    return detect_arg_error(format!("unknown observed_via: {other}"));
                }
            };
            let mut input = String::new();
            std::io::Read::take(std::io::stdin(), 16 * 1024 * 1024 + 1)
                .read_to_string(&mut input)
                .map_err(|e| format!("stdin: {e}"))?;
            if input.len() as u64 > 16 * 1024 * 1024 {
                return Err("stdin payload too large".to_string());
            }
            let payload: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| format!("payload json: {e}"))?;
            for event in events::detect_all_from_payload(runtime, via, &payload) {
                println!(
                    "{}",
                    serde_json::to_string(&event).map_err(|e| e.to_string())?
                );
            }
            Ok(())
        }
        Command::Perf { sizes, iterations } => {
            let result = perf::run(&sizes, iterations)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            Ok(())
        }
    }
}

/// Argument-validation failures on stdin-driven commands must still
/// consume (bounded) stdin: the caller writes its payload before reading
/// our exit status, and exiting early surfaces as EPIPE on their side.
fn detect_arg_error(err: String) -> Result<(), String> {
    let mut sink = Vec::new();
    let _ = std::io::Read::take(std::io::stdin(), 16 * 1024 * 1024).read_to_end(&mut sink);
    Err(err)
}

fn roots_or_default(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    crate::discovery::resolve_roots(roots, crate::discovery::default_roots)
}

fn graph_for(roots: Vec<PathBuf>) -> Result<crate::resolver::Graph, String> {
    Ok(build(discover(&roots)))
}

pub fn resolve_target(graph: &crate::resolver::Graph, target: &str) -> ResolveResult {
    if let Some(indexes) = graph.by_name.get(target) {
        return ResolveResult::Name {
            ambiguous: indexes.len() > 1,
            skills: indexes.iter().map(|i| graph.skills[*i].summary()).collect(),
        };
    }
    // `file://` targets (RFC 8089) map to a local path; only the empty or
    // `localhost` authority is local. Lookup still goes through by_path,
    // so resolution stays bounded to discovered skills.
    let raw = if let Some(path) = file_uri_to_path(target) {
        path
    } else if target.starts_with("file://") {
        return ResolveResult::Unresolved {
            target: target.to_string(),
        };
    } else {
        std::path::PathBuf::from(target)
    };
    // by_path keys are absolute (discovery canonicalizes roots); a
    // relative or non-normalized target must be absolutized first or it
    // can never match.
    let abs = if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&raw))
            .unwrap_or(raw)
    };
    let mut candidates = vec![crate::resolver::normalize_path(&abs)];
    if let Ok(canon) = std::fs::canonicalize(&abs) {
        let p = crate::resolver::normalize_path(&canon);
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    }
    // A directory target completes to <dir>/SKILL.md, mirroring the
    // resolver's directory completion for Markdown links.
    let skill_md: Vec<PathBuf> = candidates
        .iter()
        .map(|dir| dir.join("SKILL.md"))
        .filter(|p| !candidates.contains(p))
        .collect();
    candidates.extend(skill_md);
    for path in candidates {
        if let Some(index) = graph.by_path.get(&path) {
            return ResolveResult::Path {
                skill: graph.skills[*index].summary(),
            };
        }
    }
    ResolveResult::Unresolved {
        target: target.to_string(),
    }
}

/// Map a `file://` URI to a local path (RFC 8089). Only the empty or
/// `localhost` authority denotes a local file; remote authorities,
/// malformed percent-encoding, and non-UTF-8 paths yield `None`.
fn file_uri_to_path(target: &str) -> Option<PathBuf> {
    let rest = target.strip_prefix("file://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, "/"),
    };
    if !authority.is_empty() && authority != "localhost" {
        return None;
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            decoded.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

fn print_result(value: impl serde::Serialize, format: &str) -> Result<(), String> {
    match format {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        ),
        "human" => println!("{}", human(value)),
        _ => return Err(format!("unsupported format: {format}")),
    }
    Ok(())
}

fn human(value: impl serde::Serialize) -> String {
    let value = serde_json::to_value(value).unwrap_or_default();
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("skill").or(Some(item)))
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(ref map) => map
            .get("skills")
            .and_then(|s| s.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_default()),
        _ => serde_json::to_string_pretty(&value).unwrap_or_default(),
    }
}
