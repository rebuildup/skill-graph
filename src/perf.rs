use crate::discovery::discover;
use crate::resolver::build;
use crate::search::recommend;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const QUERY: &str = "rust cli implementation cargo checks";
const CONTEXT: &str = "module boundaries verification harness";
const PROCESS_SAMPLES: usize = 5;

pub fn run(sizes: &[usize], iterations: usize) -> Result<Value, String> {
    if sizes.is_empty() {
        return Err("perf requires at least one size".to_string());
    }
    if iterations == 0 {
        return Err("perf iterations must be > 0".to_string());
    }
    if sizes.len() > 16 || sizes.iter().any(|&s| s > 10_000) {
        return Err("perf sizes are capped at 16 entries of <= 10000".to_string());
    }
    if iterations > 1_000 {
        return Err("perf iterations are capped at 1000".to_string());
    }
    // Bound total work: corpus generation is O(sum of sizes) skill
    // parses and benchmarking is O(sizes × iterations) recommend()
    // calls over up-to-10k-skill graphs. Without a product cap the
    // flags alone permit ~160k recommendations.
    if sizes.iter().sum::<usize>() > 50_000 {
        return Err("perf total corpus capped at 50000 skills".to_string());
    }
    if sizes.len().saturating_mul(iterations) > 2_000 {
        return Err("perf total recommend calls capped at 2000 (sizes × iterations)".to_string());
    }
    let mut results = Vec::new();
    for &size in sizes {
        if size == 0 {
            return Err("perf size must be > 0".to_string());
        }
        results.push(run_size(size, iterations)?);
    }
    Ok(json!({ "results": results }))
}

fn run_size(size: usize, iterations: usize) -> Result<Value, String> {
    let root = corpus_root(size)?;
    // corpus_root keep()s the TempDir, so any error below would leak the
    // corpus — remove it on the failure path explicitly.
    match run_size_inner(&root, size, iterations) {
        Ok(value) => Ok(value),
        Err(err) => {
            let _ = fs::remove_dir_all(root);
            Err(err)
        }
    }
}

fn run_size_inner(root: &PathBuf, size: usize, iterations: usize) -> Result<Value, String> {
    let cold_started = Instant::now();
    let discovery = discover(std::slice::from_ref(root));
    let cold_discover = cold_started.elapsed();
    let build_started = Instant::now();
    let graph = build(discovery);
    let cold_build = build_started.elapsed();
    let cold_total = cold_started.elapsed();

    let mut warm_samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let recs = recommend(&graph, QUERY, CONTEXT, 5);
        warm_samples.push(started.elapsed());
        std::hint::black_box(recs);
    }

    let mut process_samples = Vec::with_capacity(PROCESS_SAMPLES);
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let root_arg = root
        .to_str()
        .ok_or_else(|| "non-utf8 perf root".to_string())?;
    for _ in 0..PROCESS_SAMPLES {
        let started = Instant::now();
        let output = Command::new(&exe)
            .args([
                "recommend",
                QUERY,
                "--context",
                CONTEXT,
                "--root",
                root_arg,
                "--max",
                "5",
                "--format",
                "json",
            ])
            // resolve_roots gives SKILL_GRAPH_ROOTS precedence over
            // --root; a set env var would silently measure the wrong corpus.
            .env_remove("SKILL_GRAPH_ROOTS")
            .output()
            .map_err(|err| format!("spawn perf cli: {err}"))?;
        if !output.status.success() {
            return Err(format!(
                "perf cli failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        process_samples.push(started.elapsed());
        std::hint::black_box(&output.stdout);
    }

    let cleanup_ok = fs::remove_dir_all(root).is_ok();
    let recommended_count = recommend(&graph, QUERY, CONTEXT, 5).len();

    Ok(json!({
        "skills": size,
        "cold_discover_ms": millis(cold_discover),
        "cold_build_ms": millis(cold_build),
        "cold_total_ms": millis(cold_total),
        "warm_recommend_p50_ms": percentile(&warm_samples, 0.50),
        "warm_recommend_p95_ms": percentile(&warm_samples, 0.95),
        "warm_recommend_max_ms": millis(warm_samples.iter().max().copied().unwrap_or_default()),
        "process_p50_ms": percentile(&process_samples, 0.50),
        "process_p95_ms": percentile(&process_samples, 0.95),
        "process_samples": process_samples.len(),
        "recommended_count": recommended_count,
        "cleanup_ok": cleanup_ok,
    }))
}

fn corpus_root(size: usize) -> Result<PathBuf, String> {
    // Private tempdir: a predictable shared-/tmp path would let another
    // local user pre-plant SKILL.md symlinks and redirect the writes.
    let tmp = tempfile::Builder::new()
        .prefix("skill-graph-perf-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    // Keep the TempDir guard until success: keep()ing up front would leak
    // the corpus into /tmp when corpus generation fails partway.
    let root = tmp.path();
    for index in 0..size {
        let name = format!("perf-skill-{index:04}");
        let description = if index == 0 {
            "Use for rust cli implementation cargo checks module boundaries and verification harness repair.".to_string()
        } else {
            format!(
                "Use for synthetic benchmark skill {index:04} with unique terms alpha{index} beta{index} gamma{index}."
            )
        };
        let requires = if index == 0 {
            "metadata:\n  requires-skills: perf-skill-0001\n"
        } else {
            ""
        };
        let body = format!(
            "# {name}\n\nBenchmark body for {name}. {}\n",
            "lorem ipsum dolor sit amet consectetur adipiscing elit sed do ".repeat(8)
        );
        let content =
            format!("---\nname: {name}\ndescription: {description}\n{requires}---\n\n{body}");
        let dir = root.join(&name);
        fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        fs::write(dir.join("SKILL.md"), content).map_err(|err| err.to_string())?;
    }
    Ok(tmp.keep())
}

fn percentile(samples: &[Duration], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut values: Vec<u128> = samples.iter().map(|duration| duration.as_nanos()).collect();
    values.sort_unstable();
    let index = ((p * values.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[index] as f64 / 1_000_000.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_nanos() as f64 / 1_000_000.0
}
