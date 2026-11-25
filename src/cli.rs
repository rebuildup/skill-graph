use crate::discovery::{discover, output};
use crate::eval;
use crate::model::ResolveResult;
use crate::perf;
use crate::resolver::build;
use crate::search::recommend;
use clap::{Parser, Subcommand};
use serde_json::json;
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
        query: String,
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
    },
    Perf {
        #[arg(long, value_delimiter = ',', default_value = "50,100,200,500")]
        sizes: Vec<usize>,
        #[arg(long, default_value_t = 30)]
        iterations: usize,
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
            context,
            root,
            max,
            format,
        } => {
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
        Command::Bench { dataset } => {
            let result = eval::run(&dataset)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
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

fn roots_or_default(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    if roots.is_empty() {
        crate::discovery::default_roots()
    } else {
        roots
    }
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
    let path = crate::resolver::normalize_path(&std::path::PathBuf::from(target));
    if let Some(index) = graph.by_path.get(&path) {
        return ResolveResult::Path {
            skill: graph.skills[*index].summary(),
        };
    }
    ResolveResult::Unresolved {
        target: target.to_string(),
    }
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
