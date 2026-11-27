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

/// Dataset bound: `bench` is a maintainer-invoked dev tool, but an
/// unbounded JSONL read is the one remaining uncapped CLI input — a
/// multi-GB or hostile dataset would OOM the process (#144 r17).
const MAX_DATASET_BYTES: u64 = 64 * 1024 * 1024;
/// Case-count bound: the byte cap limits input size, not case count —
/// ~1.5M minimal cases would each run `recommend` and retain a result
/// object, hanging or OOMing the run (#144 r18).
const MAX_EVAL_CASES: usize = 10_000;
/// Distinct `skill_root` graphs retained at once. Each Graph can itself
/// be large, and a dataset naming thousands of roots would otherwise
/// build and retain all of them simultaneously (#144 r18).
const MAX_EVAL_GRAPHS: usize = 32;
/// Per-case expected/forbidden list bound: hits are computed against at
/// most `fetch` recommended names, so longer lists only burn CPU.
const MAX_EVAL_EXPECTED: usize = 1024;

/// `max` is the primary recommendation cutoff scored by `required_recall`.
/// It defaults to the top-N the runtime actually injects (hook/plugin
/// surface top-3); recall@3 and recall@5 are additionally reported from
/// the same `recommend` pass so metric drift stays visible.
pub fn run(dataset: &Path, max: usize) -> Result<Value, String> {
    use std::io::Read;
    let file = fs::File::open(dataset).map_err(|e| format!("read {}: {e}", dataset.display()))?;
    let mut text = String::new();
    file.take(MAX_DATASET_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("read {}: {e}", dataset.display()))?;
    if text.len() as u64 > MAX_DATASET_BYTES {
        return Err(format!(
            "{}: dataset exceeds {} bytes",
            dataset.display(),
            MAX_DATASET_BYTES
        ));
    }
    let mut cases = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let case: Case = serde_json::from_str(line)
            .map_err(|e| format!("{}:{}: {e}", dataset.display(), line_no + 1))?;
        for (what, list) in [
            ("expected_required", &case.expected_required),
            ("expected_optional", &case.expected_optional),
            ("forbidden", &case.forbidden),
        ] {
            if list.len() > MAX_EVAL_EXPECTED {
                return Err(format!(
                    "case {}: {what} exceeds {} entries",
                    case.id, MAX_EVAL_EXPECTED
                ));
            }
        }
        if cases.len() >= MAX_EVAL_CASES {
            return Err(format!(
                "{}: dataset exceeds {} cases",
                dataset.display(),
                MAX_EVAL_CASES
            ));
        }
        cases.push(case);
    }

    // `skill_root` is repository-relative per evals/dataset/schema.md.
    // Resolve it against the repo root (nearest ancestor of the dataset
    // containing Cargo.toml), falling back to the dataset's parent —
    // never silently against an arbitrary cwd. Absolutize first: a
    // relative dataset path would otherwise stop the ancestor walk at
    // the dataset directory itself.
    let dataset_abs = std::path::absolute(dataset).unwrap_or_else(|_| dataset.to_path_buf());
    let repo_root = dataset_abs
        .parent()
        .and_then(|d| d.ancestors().find(|a| a.join("Cargo.toml").exists()))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dataset_abs.parent().unwrap_or(Path::new(".")).to_path_buf());

    // Canonical repo root for the containment check below: a committed
    // symlink inside the repo could otherwise point skill_root outside
    // it while passing the lexical checks.
    let repo_canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.clone());

    let mut graphs: HashMap<PathBuf, crate::resolver::Graph> = HashMap::new();
    // Insertion-ordered LRU for the graph cache: datasets cluster cases
    // by root, so the working set stays tiny while the retained count
    // stays bounded (#144 r18).
    let mut graph_lru: std::collections::VecDeque<PathBuf> = std::collections::VecDeque::new();
    let mut discovery_errors: Vec<String> = Vec::new();
    let mut results = Vec::new();
    let mut totals = json!({
        "cases": cases.len(),
        "required_hits": 0,
        "required_hits_at3": 0,
        "required_hits_at5": 0,
        "required_total": 0,
        "forbidden_hits": 0,
        "optional_hits": 0,
        "optional_total": 0,
        "recommended_total": 0,
        "empty_recommendations": 0,
        "elapsed_ms": 0,
        "max": max,
    });
    let started = Instant::now();

    for case in cases {
        // Dataset contract (evals/dataset/schema.md): skill_root stays
        // inside the repository — no absolute paths, no `..` segments.
        // A violating case is a dataset bug; fail loudly rather than
        // scanning outside the repo.
        if case.skill_root.is_absolute()
            || case
                .skill_root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "case {}: skill_root must be a repository-relative path without '..': {}",
                case.id,
                case.skill_root.display()
            ));
        }
        let root = repo_root.join(&case.skill_root);
        // Lexical checks pass for a symlinked path; resolve it so a
        // repo-committed link cannot smuggle an outside-repo scan.
        // A root that does not exist falls through — discovery reports
        // it as an error either way.
        if let Ok(canon) = root.canonicalize()
            && !canon.starts_with(&repo_canon)
        {
            return Err(format!(
                "case {}: skill_root resolves outside the repository: {}",
                case.id,
                case.skill_root.display()
            ));
        }
        if graphs.contains_key(&root) {
            // Refresh recency: move the key to the back of the LRU.
            if let Some(pos) = graph_lru.iter().position(|p| p == &root) {
                graph_lru.remove(pos);
            }
            graph_lru.push_back(root.clone());
        } else {
            if graphs.len() >= MAX_EVAL_GRAPHS
                && let Some(oldest) = graph_lru.pop_front()
            {
                graphs.remove(&oldest);
            }
            let discovery = discover(std::slice::from_ref(&root));
            discovery_errors.extend(discovery.errors.iter().cloned());
            graphs.insert(root.clone(), build(discovery));
            graph_lru.push_back(root.clone());
        }
        let graph = graphs
            .get(&root)
            .ok_or_else(|| "missing graph".to_string())?;
        // One recommend() pass deep enough for the primary cutoff and the
        // diagnostic recall@3/recall@5 cutoffs; ranking is a prefix, so
        // slicing names_all reproduces each cutoff exactly.
        let fetch = max.max(5);
        let recs = recommend(graph, &case.query, &case.context, fetch);
        let names_all: Vec<String> = recs.iter().map(|r| r.skill.name.clone()).collect();
        let names: Vec<String> = names_all.iter().take(max).cloned().collect();
        let hits_at = |cutoff: usize| -> usize {
            case.expected_required
                .iter()
                .filter(|n| names_all.iter().take(cutoff).any(|m| m == *n))
                .count()
        };
        let required_hits = case
            .expected_required
            .iter()
            .filter(|n| names.contains(n))
            .count();
        let required_hits_at3 = hits_at(3);
        let required_hits_at5 = hits_at(5);
        let forbidden_hits = case.forbidden.iter().filter(|n| names.contains(n)).count();
        let optional_hits = case
            .expected_optional
            .iter()
            .filter(|n| names.contains(n))
            .count();
        if names.is_empty() {
            totals["empty_recommendations"] =
                json!(totals["empty_recommendations"].as_u64().unwrap_or(0) + 1);
        }
        totals["required_hits"] =
            json!(totals["required_hits"].as_u64().unwrap_or(0) + required_hits as u64);
        totals["required_hits_at3"] =
            json!(totals["required_hits_at3"].as_u64().unwrap_or(0) + required_hits_at3 as u64);
        totals["required_hits_at5"] =
            json!(totals["required_hits_at5"].as_u64().unwrap_or(0) + required_hits_at5 as u64);
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
            "required_hits_at3": required_hits_at3,
            "required_hits_at5": required_hits_at5,
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
    for cutoff in [3usize, 5usize] {
        let hits = totals[format!("required_hits_at{cutoff}")]
            .as_u64()
            .unwrap_or(0);
        totals[format!("required_recall_at{cutoff}")] = json!(if required_total == 0 {
            1.0
        } else {
            hits as f64 / required_total as f64
        });
    }
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

    let mut out = json!({ "summary": totals, "cases": results });
    if !discovery_errors.is_empty() {
        out["discovery_errors"] = json!(discovery_errors);
    }
    Ok(out)
}
