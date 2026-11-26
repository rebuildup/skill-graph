use clap::Parser;
use skill_graph::discovery::discover;
use skill_graph::resolver::{Graph, build};
use skill_graph::search::recommend;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn text_only(graph: &Graph) -> Graph {
    let mut skills = graph.skills.clone();
    for skill in &mut skills {
        skill.links.clear();
    }
    Graph {
        skills,
        by_name: graph.by_name.clone(),
        by_path: graph.by_path.clone(),
        backlinks: HashMap::new(),
        ambiguous_names: graph.ambiguous_names.clone(),
    }
}

fn recommend_names(
    root: &Path,
    query: &str,
    context: &str,
    max: usize,
    text_only_mode: bool,
) -> Result<Vec<String>, String> {
    let graph = build(discover(&[root.to_path_buf()]));
    let graph = if text_only_mode {
        text_only(&graph)
    } else {
        graph
    };
    Ok(recommend(&graph, query, context, max)
        .iter()
        .map(|rec| rec.skill.name.clone())
        .collect())
}

#[derive(Parser)]
struct Args {
    query: String,
    #[arg(long)]
    root: PathBuf,
    #[arg(long, default_value_t = 3)]
    max: usize,
    #[arg(long)]
    context: Option<String>,
    #[arg(long)]
    text_only: bool,
}

fn main() {
    let args = Args::parse();
    match recommend_names(
        &args.root,
        &args.query,
        args.context.as_deref().unwrap_or(""),
        args.max,
        args.text_only,
    ) {
        Ok(names) => println!(
            "{}",
            serde_json::to_string(&names).expect("serialize names")
        ),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
