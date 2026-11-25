use crate::discovery::discover;
use crate::resolver::build;
use crate::search::recommend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    #[serde(default)]
    timing: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    case_kind: Option<String>,
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

    let mut graphs: HashMap<PathBuf, crate::resolver::Graph> = HashMap::new();
    let mut results = Vec::new();
    let mut totals = json!({
        "cases": cases.len(),
        "required_hits": 0,
        "required_total": 0,
        "forbidden_hits": 0,
        "optional_hits": 0,
        "optional_total": 0,
        "recommended_total": 0,
        "empty_recommendations": 0,
        "elapsed_ms": 0,
    });
    let started = Instant::now();

    for case in cases {
        let cwd_root = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&case.skill_root);
        let root = if case.skill_root.is_absolute() {
            case.skill_root.clone()
        } else if cwd_root.exists() {
            cwd_root
        } else {
            dataset
                .parent()
                .unwrap_or(Path::new("."))
                .join(case.skill_root)
        };
        if !graphs.contains_key(&root) {
            graphs.insert(root.clone(), build(discover(std::slice::from_ref(&root))));
        }
        let graph = graphs
            .get(&root)
            .ok_or_else(|| "missing graph".to_string())?;
        let recs = recommend(graph, &case.query, &case.context, 5);
        let names: Vec<String> = recs.iter().map(|r| r.skill.name.clone()).collect();
        let required_hits = case
            .expected_required
            .iter()
            .filter(|n| names.contains(n))
            .count();
        let forbidden_hits = case.forbidden.iter().filter(|n| names.contains(n)).count();
        let optional_hits = case
            .expected_optional
            .iter()
            .filter(|n| names.contains(n))
            .count();
        if recs.is_empty() {
            totals["empty_recommendations"] =
                json!(totals["empty_recommendations"].as_u64().unwrap_or(0) + 1);
        }
        totals["required_hits"] =
            json!(totals["required_hits"].as_u64().unwrap_or(0) + required_hits as u64);
        totals["required_total"] = json!(
            totals["required_total"].as_u64().unwrap_or(0) + case.expected_required.len() as u64
        );
        totals["forbidden_hits"] =
            json!(totals["forbidden_hits"].as_u64().unwrap_or(0) + forbidden_hits as u64);
        totals["optional_hits"] =
            json!(totals["optional_hits"].as_u64().unwrap_or(0) + optional_hits as u64);
        totals["optional_total"] = json!(
            totals["optional_total"].as_u64().unwrap_or(0) + case.expected_optional.len() as u64
        );
        let recommended_count = names.len();
        totals["recommended_total"] =
            json!(totals["recommended_total"].as_u64().unwrap_or(0) + recommended_count as u64);
        results.push(json!({
            "id": case.id,
            "recommended": names,
            "recommended_count": recommended_count,
            "required_hits": required_hits,
            "required_total": case.expected_required.len(),
            "forbidden_hits": forbidden_hits,
            "optional_hits": optional_hits,
            "timing": case.timing,
            "rationale": case.rationale,
            "case_kind": case.case_kind,
        }));
    }

    totals["elapsed_ms"] = json!(started.elapsed().as_millis());
    let required_total = totals["required_total"].as_u64().unwrap_or(0);
    let required_hits = totals["required_hits"].as_u64().unwrap_or(0);
    totals["required_recall"] = json!(if required_total == 0 {
        1.0
    } else {
        required_hits as f64 / required_total as f64
    });
    let recommended_total = totals["recommended_total"].as_u64().unwrap_or(0);
    let optional_hits = totals["optional_hits"].as_u64().unwrap_or(0);
    totals["precision_required"] = json!(if recommended_total == 0 {
        1.0
    } else {
        required_hits as f64 / recommended_total as f64
    });
    totals["precision_acceptable"] = json!(if recommended_total == 0 {
        1.0
    } else {
        (required_hits + optional_hits) as f64 / recommended_total as f64
    });
    totals["cases"] = json!(results.len());

    Ok(json!({ "summary": totals, "cases": results }))
}
