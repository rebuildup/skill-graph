use crate::discovery::discover;
use crate::model::LinkKind;
use crate::resolver::{Graph, build};
use crate::search::recommend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const ARMS: [&str; 5] = [
    "baseline",
    "compact_start",
    "requires_from_baseline",
    "reference_from_baseline",
    "compact_plus_reference",
];

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
    reference_added: usize,
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

    let mut graphs: HashMap<PathBuf, Graph> = HashMap::new();
    let mut totals: HashMap<&'static str, ArmTotals> = HashMap::new();
    for arm in ARMS {
        totals.insert(arm, ArmTotals::default());
    }
    let mut results = Vec::new();

    for case in cases {
        let root = case_root(dataset, &case);
        if !graphs.contains_key(&root) {
            graphs.insert(root.clone(), build(discover(std::slice::from_ref(&root))));
        }
        let graph = graphs.get(&root).ok_or("missing graph")?;

        let baseline = names(&recommend(graph, &case.query, &case.context, 5));
        let compact = names(&recommend(graph, "", &case.context, 3));
        let baseline_requires = reference_names(graph, &baseline, false);
        let baseline_requires_arm = union_names(&baseline, &baseline_requires);
        let baseline_refs = reference_names(graph, &baseline, true);
        let baseline_arm = union_names(&baseline, &baseline_refs);
        let compact_refs = reference_names(graph, &compact, true);
        let compact_arm = union_names(&compact, &compact_refs);

        let arms = [
            ("baseline", baseline, 0usize),
            ("compact_start", compact, 0),
            (
                "requires_from_baseline",
                baseline_requires_arm,
                baseline_requires.len(),
            ),
            ("reference_from_baseline", baseline_arm, baseline_refs.len()),
            ("compact_plus_reference", compact_arm, compact_refs.len()),
        ];

        let mut case_result = serde_json::Map::new();
        for (arm, names, added) in arms {
            let total = totals.get_mut(arm).ok_or("missing arm")?;
            total.cases += 1;
            total.required_total += case.expected_required.len();
            total.optional_total += case.expected_optional.len();
            total.recommended_total += names.len();
            total.reference_added += added;
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
                    "reference_added": added,
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

fn names(recs: &[crate::model::Recommendation]) -> Vec<String> {
    recs.iter().map(|rec| rec.skill.name.clone()).collect()
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

fn union_names(base: &[String], added: &[String]) -> Vec<String> {
    let mut out = base.to_vec();
    for name in added {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

fn reference_names(graph: &Graph, base: &[String], include_markdown: bool) -> Vec<String> {
    let base_set: HashSet<&str> = base.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut added = Vec::new();
    for name in base {
        let Some(indexes) = graph.by_name.get(name) else {
            continue;
        };
        for index in indexes {
            for link in &graph.skills[*index].links {
                let explicit = link.kind == LinkKind::Requires
                    || (include_markdown && link.kind == LinkKind::Markdown);
                if !explicit {
                    continue;
                }
                let Some(target) = &link.resolved_name else {
                    continue;
                };
                if base_set.contains(target.as_str()) || !seen.insert(target.clone()) {
                    continue;
                }
                added.push(target.clone());
            }
        }
    }
    added.sort();
    added
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
        "reference_added": t.reference_added,
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
