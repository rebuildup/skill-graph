use crate::discovery::discover;
use crate::resolver::{Graph, build};
use crate::search::recommend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const ARMS: [&str; 2] = ["E_text_search", "F_text_graph"];
const HOOK_MAX: usize = 3;

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    query: String,
    #[serde(default)]
    context: String,
    skill_root: PathBuf,
    #[serde(default)]
    expected_required: Vec<String>,
    #[serde(default)]
    expected_optional: Vec<String>,
    #[serde(default)]
    forbidden: Vec<String>,
}

#[derive(Default)]
struct ArmTotals {
    cases: usize,
    required_hits: usize,
    required_total: usize,
    forbidden_hits: usize,
    optional_hits: usize,
    optional_total: usize,
    recommended_total: usize,
    empty_recommendations: usize,
    elapsed_us: Vec<u128>,
}

pub fn run(dataset: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(dataset).map_err(|e| format!("read {}: {e}", dataset.display()))?;
    let mut cases = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let case: Case = serde_json::from_str(line)
            .map_err(|e| format!("{}:{}: {e}", dataset.display(), line_no + 1))?;
        cases.push(case);
    }

    let mut graphs: HashMap<PathBuf, (Graph, Graph)> = HashMap::new();
    let mut totals: HashMap<&'static str, ArmTotals> = HashMap::new();
    for arm in ARMS {
        totals.insert(arm, ArmTotals::default());
    }
    let mut results = Vec::new();

    for case in cases {
        let root = case_root(dataset, &case);
        if !graphs.contains_key(&root) {
            let full = build(discover(std::slice::from_ref(&root)));
            let text_only = text_only(&full);
            graphs.insert(root.clone(), (full, text_only));
        }
        let (full, text_only) = graphs.get(&root).ok_or("missing graph")?;

        let mut case_result = serde_json::Map::new();
        for (arm, graph) in [("E_text_search", text_only), ("F_text_graph", full)] {
            let started = Instant::now();
            let recs = recommend(graph, &case.query, &case.context, HOOK_MAX);
            let elapsed_us = started.elapsed().as_micros();
            let names: Vec<String> = recs.iter().map(|rec| rec.skill.name.clone()).collect();

            let total = totals.get_mut(arm).ok_or("missing arm")?;
            total.cases += 1;
            total.required_total += case.expected_required.len();
            total.optional_total += case.expected_optional.len();
            total.recommended_total += names.len();
            total.elapsed_us.push(elapsed_us);
            if names.is_empty() {
                total.empty_recommendations += 1;
            }
            let metrics = case_metrics(&names, &case);
            total.required_hits += metrics.0;
            total.forbidden_hits += metrics.1;
            total.optional_hits += metrics.2;
            case_result.insert(
                arm.to_string(),
                json!({
                    "recommended": names,
                    "recommended_count": names.len(),
                    "required_hits": metrics.0,
                    "forbidden_hits": metrics.1,
                    "optional_hits": metrics.2,
                    "elapsed_us": elapsed_us,
                }),
            );
        }
        results.push(json!({ "id": case.id, "arms": Value::Object(case_result) }));
    }

    let mut summaries = serde_json::Map::new();
    for arm in ARMS {
        let t = totals.get(arm).ok_or("missing arm")?;
        summaries.insert(arm.to_string(), finalize(t));
    }
    Ok(json!({ "summary": Value::Object(summaries), "cases": results }))
}

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

pub fn recommend_names(
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

fn case_metrics(names: &[String], case: &Case) -> (usize, usize, usize) {
    let required_hits = case
        .expected_required
        .iter()
        .filter(|name| names.contains(name))
        .count();
    let forbidden_hits = case
        .forbidden
        .iter()
        .filter(|name| names.contains(name))
        .count();
    let optional_hits = case
        .expected_optional
        .iter()
        .filter(|name| names.contains(name))
        .count();
    (required_hits, forbidden_hits, optional_hits)
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn finalize(t: &ArmTotals) -> Value {
    let required_recall = if t.required_total == 0 {
        1.0
    } else {
        t.required_hits as f64 / t.required_total as f64
    };
    let precision_required = if t.recommended_total == 0 {
        1.0
    } else {
        t.required_hits as f64 / t.recommended_total as f64
    };
    let precision_acceptable = if t.recommended_total == 0 {
        1.0
    } else {
        (t.required_hits + t.optional_hits) as f64 / t.recommended_total as f64
    };
    let mut sorted = t.elapsed_us.clone();
    sorted.sort_unstable();
    let total_us: u128 = t.elapsed_us.iter().sum();
    let avg_us = if sorted.is_empty() {
        0
    } else {
        total_us / sorted.len() as u128
    };
    json!({
        "cases": t.cases,
        "required_hits": t.required_hits,
        "required_total": t.required_total,
        "required_recall": required_recall,
        "forbidden_hits": t.forbidden_hits,
        "optional_hits": t.optional_hits,
        "optional_total": t.optional_total,
        "recommended_total": t.recommended_total,
        "precision_required": precision_required,
        "precision_acceptable": precision_acceptable,
        "empty_recommendations": t.empty_recommendations,
        "recommend_us_avg": avg_us,
        "recommend_us_p50": percentile(&sorted, 0.50),
        "recommend_us_p95": percentile(&sorted, 0.95),
        "recommend_us_max": sorted.last().copied().unwrap_or(0),
    })
}

fn case_root(dataset: &Path, case: &Case) -> PathBuf {
    let cwd_root = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(&case.skill_root);
    if case.skill_root.is_absolute() {
        case.skill_root.clone()
    } else if cwd_root.exists() {
        cwd_root
    } else {
        dataset
            .parent()
            .unwrap_or(Path::new("."))
            .join(&case.skill_root)
    }
}
