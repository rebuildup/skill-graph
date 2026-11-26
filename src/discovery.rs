use crate::model::{DiscoveryOutput, Skill};
use crate::parser::parse_skill;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct Discovery {
    pub skills: Vec<Skill>,
    pub errors: Vec<String>,
}

pub fn default_roots() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut roots = vec![
        cwd.join("skills"),
        cwd.join(".agents/skills"),
        cwd.join(".claude/skills"),
        cwd.join(".opencode/skills"),
        cwd.join(".devin/skills"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.extend([
            home.join(".agents/skills"),
            home.join(".claude/skills"),
            home.join(".config/opencode/skills"),
        ]);
    }
    roots
}

/// Outcome of parsing `SKILL_GRAPH_ROOTS`.
///
/// The variable is an operator-controlled restriction: once set to a
/// non-empty value, its entries — valid or not — define the ENTIRE
/// search scope. Entries that do not resolve are diagnosed by
/// `discover` ("root not found or unreadable") instead of being silently
/// dropped, and when none resolve the result is an empty scan rather
/// than a silent widening to explicit arguments or default roots.
pub enum EnvRoots {
    /// Variable unset or set to an empty value: no operator restriction.
    Absent,
    /// Variable present with at least one separator-delimited entry.
    /// Entries are passed through verbatim (including non-UTF-8 paths);
    /// validity is determined downstream by canonicalization.
    Restricted(Vec<PathBuf>),
}

/// Parse `SKILL_GRAPH_ROOTS` using the platform path-list separator
/// (`:` on Unix). Presence is detected via `var_os` so a non-UTF-8
/// value still counts as an explicit restriction rather than falling
/// back to wider defaults.
pub fn env_roots() -> EnvRoots {
    let Some(spec) = std::env::var_os("SKILL_GRAPH_ROOTS") else {
        return EnvRoots::Absent;
    };
    if spec.is_empty() {
        return EnvRoots::Absent;
    }
    let roots: Vec<PathBuf> = std::env::split_paths(&spec).collect();
    if roots.is_empty() {
        // Non-empty spec that yields no entries (e.g. only separators):
        // still an explicit restriction — resolve to an empty scope.
        return EnvRoots::Restricted(Vec::new());
    }
    EnvRoots::Restricted(roots)
}

/// Shared root resolution for every entry point (CLI, hook, MCP, plugin):
/// a non-empty `SKILL_GRAPH_ROOTS` wins over explicit arguments, which
/// win over the caller-provided fallback (runtime- or CLI-specific
/// defaults). The operator-controlled env var therefore cannot be
/// overridden by an agent-supplied argument — see ADR-0005 — and an
/// invalid or empty restriction degrades to an empty/diagnosed scan,
/// never to a wider scope — see issue #177.
pub fn resolve_roots(
    explicit: Vec<PathBuf>,
    fallback: impl FnOnce() -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    match env_roots() {
        EnvRoots::Restricted(roots) => roots,
        EnvRoots::Absent => {
            if !explicit.is_empty() {
                explicit
            } else {
                fallback()
            }
        }
    }
}

/// Upper bound on filesystem entries scanned per root. MCP callers can
/// pass arbitrary roots (e.g. `/`); without a cap a single call walks
/// the whole filesystem before output truncation applies.
const MAX_DISCOVER_ENTRIES: usize = 100_000;

/// Upper bounds on accepted skills and their cumulative bytes. Each
/// SKILL.md may be up to 1 MiB and `Skill` retains the full body text;
/// without caps a hostile or pathological tree could exhaust memory.
const MAX_SKILLS: usize = 10_000;
const MAX_SKILL_BYTES: u64 = 512 * 1024 * 1024;

pub fn discover(roots: &[PathBuf]) -> Discovery {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut total_bytes = 0u64;
    let mut capacity_truncated = false;

    for requested in roots {
        if capacity_truncated {
            break;
        }
        let Ok(root) = std::fs::canonicalize(requested) else {
            errors.push(format!(
                "root not found or unreadable: {}",
                requested.display()
            ));
            continue;
        };
        if !root.is_dir() {
            // An existing non-directory root (e.g. a file path) is an
            // operator/agent error worth surfacing, not a silent skip.
            errors.push(format!("root is not a directory: {}", requested.display()));
            continue;
        }
        if !seen_roots.insert(root.clone()) {
            continue;
        }
        // Sort entries so discovery order — and therefore Graph.skills
        // index order, the deterministic body-scan budget order in
        // recommend(), and equal-score output ordering — does not depend
        // on filesystem directory order.
        let mut entries = WalkDir::new(&root)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter();
        let mut consumed = 0usize;
        let mut walk_errors = 0usize;
        for entry in entries.by_ref().take(MAX_DISCOVER_ENTRIES) {
            consumed += 1;
            if capacity_truncated {
                break;
            }
            // Surface IO errors (permission-denied dirs etc.) instead of
            // silently reporting a partial scan as clean.
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    if walk_errors < 100 {
                        errors.push(format!(
                            "{}: walk error at {}: {err}",
                            root.display(),
                            err.path()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        ));
                    }
                    walk_errors += 1;
                    continue;
                }
            };
            if entry.file_type().is_symlink() || entry.file_name() != "SKILL.md" {
                continue;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Contained/overlapping roots can surface the same SKILL.md
            // twice; count each file once.
            let Ok(canonical) = std::fs::canonicalize(path) else {
                continue;
            };
            if !seen_files.insert(canonical) {
                continue;
            }
            let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if skills.len() >= MAX_SKILLS || total_bytes + file_bytes > MAX_SKILL_BYTES {
                capacity_truncated = true;
                break;
            }
            match parse_skill(path, &root) {
                Ok(skill) => {
                    total_bytes += file_bytes;
                    skills.push(skill);
                }
                Err(err) => errors.push(err),
            }
        }
        // `consumed` counts every iterator item including walk errors;
        // truncation is about consumed budget, not successfully read entries.
        if consumed >= MAX_DISCOVER_ENTRIES && entries.next().is_some() {
            errors.push(format!(
                "{}: entry scan truncated at {} entries",
                root.display(),
                MAX_DISCOVER_ENTRIES
            ));
        }
    }

    if capacity_truncated {
        errors.push(format!(
            "discovery truncated: skill limit {} or total size limit {} MiB reached",
            MAX_SKILLS,
            MAX_SKILL_BYTES / (1024 * 1024)
        ));
    }

    Discovery { skills, errors }
}

pub fn output(discovery: &Discovery) -> DiscoveryOutput {
    DiscoveryOutput {
        skills: discovery.skills.iter().map(Skill::summary).collect(),
        errors: discovery.errors.clone(),
    }
}

pub fn path_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}
