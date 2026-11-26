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

/// Parse `SKILL_GRAPH_ROOTS` (colon-separated, existing dirs only).
/// Returns `None` when unset or nothing usable remains.
pub fn env_roots() -> Option<Vec<PathBuf>> {
    let spec = std::env::var("SKILL_GRAPH_ROOTS").ok()?;
    let roots: Vec<PathBuf> = spec
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
    (!roots.is_empty()).then_some(roots)
}

/// Shared root resolution for every entry point (CLI, hook, MCP, plugin):
/// `SKILL_GRAPH_ROOTS` wins over explicit arguments, which win over the
/// caller-provided fallback (runtime- or CLI-specific defaults). The
/// operator-controlled env var therefore cannot be overridden by an
/// agent-supplied argument — see ADR-0005.
pub fn resolve_roots(
    explicit: Vec<PathBuf>,
    fallback: impl FnOnce() -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(roots) = env_roots() {
        return roots;
    }
    if !explicit.is_empty() {
        return explicit;
    }
    fallback()
}

/// Upper bound on filesystem entries scanned per root. MCP callers can
/// pass arbitrary roots (e.g. `/`); without a cap a single call walks
/// the whole filesystem before output truncation applies.
const MAX_DISCOVER_ENTRIES: usize = 100_000;

pub fn discover(roots: &[PathBuf]) -> Discovery {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut seen_files = HashSet::new();

    for requested in roots {
        let Ok(root) = std::fs::canonicalize(requested) else {
            errors.push(format!(
                "root not found or unreadable: {}",
                requested.display()
            ));
            continue;
        };
        if !root.is_dir() || !seen_roots.insert(root.clone()) {
            continue;
        }
        let mut entries = WalkDir::new(&root).follow_links(false).into_iter();
        let mut scanned = 0usize;
        let mut walk_errors = 0usize;
        for entry in entries.by_ref().take(MAX_DISCOVER_ENTRIES) {
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
            scanned += 1;
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
            match parse_skill(path, &root) {
                Ok(skill) => skills.push(skill),
                Err(err) => errors.push(err),
            }
        }
        if scanned >= MAX_DISCOVER_ENTRIES && entries.next().is_some() {
            errors.push(format!(
                "{}: entry scan truncated at {} entries",
                root.display(),
                MAX_DISCOVER_ENTRIES
            ));
        }
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
