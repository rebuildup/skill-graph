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
/// Global entry budget across ALL roots (#225 B03): per-root caps
/// alone let N roots multiply total work, `seen_files` growth, and
/// wall-clock time without bound. Once exhausted, remaining roots are
/// skipped with an explicit truncation notice.
const MAX_TOTAL_DISCOVER_ENTRIES: usize = 500_000;
/// Upper bound on directory depth (#225 B02). walkdir keeps one open
/// fd per directory on the current path; `max_open(usize::MAX)` alone
/// makes fd count ≈ tree depth, so an adversarial deep tree could
/// exhaust the process fd limit. Depth ≤64 bounds open fds to ~65.
/// The alternative — a small `max_open` — is WORSE: walkdir's
/// `DirList::close()` drains the closed directory's remaining entries
/// into a Vec, so a wide dir above a deep chain would be fully
/// buffered in memory. With max_depth the stack never exceeds the
/// depth bound, `close()` is never reached, per-directory iteration
/// stays lazy (fs::ReadDir yields one entry at a time), and BOTH the
/// fd and memory bounds hold simultaneously. SKILL.md files deeper
/// than the limit are reported as an explicit truncation, never
/// silently skipped.
const MAX_DISCOVER_DEPTH: usize = 64;
/// SKILL.md candidates collected across ALL roots before parsing
/// (#225 B03). Parse consumes at most MAX_SKILLS successes plus
/// MAX_SKILLS errors globally (pigeonhole), so collecting more is
/// wasted traversal — the budget is shared, not per-root.
const MAX_SKILL_CANDIDATES: usize = 2 * MAX_SKILLS;

/// Upper bounds on accepted skills and their cumulative bytes. Each
/// SKILL.md may be up to 1 MiB and `Skill` retains the full body text;
/// without caps a hostile or pathological tree could exhaust memory.
const MAX_SKILLS: usize = 10_000;
const MAX_SKILL_BYTES: u64 = 512 * 1024 * 1024;
/// Parse/read failures are bounded independently of MAX_SKILLS: they
/// consume neither the skill nor the byte budget, so an unreadable
/// corpus would otherwise grow `errors` without limit.
const MAX_DISCOVER_ERRORS: usize = 1024;

/// Every diagnostic shares one bound — root errors, walk errors,
/// parse errors, and truncation notices alike — so no input shape can
/// grow `errors` past MAX_DISCOVER_ERRORS. The last slot is reserved
/// for the latched truncation notice.
fn push_discovery_error(errors: &mut Vec<String>, msg: String, noted: &mut bool) {
    if errors.len() + 1 < MAX_DISCOVER_ERRORS {
        errors.push(msg);
    } else if !*noted {
        *noted = true;
        errors.push(format!(
            "further discovery errors truncated at {}",
            MAX_DISCOVER_ERRORS - 1
        ));
    }
}

pub fn discover(roots: &[PathBuf]) -> Discovery {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut seen_roots = HashSet::new();
    // Dedup key is a 64-bit hash of the canonical path, not the PathBuf
    // itself: `seen_files` grows once per encountered SKILL.md across
    // ALL roots, and storing full paths would let a hostile corpus
    // multiply retained memory by path length (#225 B03). A 64-bit
    // collision merely merges two files' dedup slot — the parse loop
    // still bounds total work; it cannot admit extra skills.
    let mut seen_files: HashSet<u64> = HashSet::new();
    let mut total_bytes = 0u64;
    let mut capacity_truncated = false;
    let mut errored = 0usize;
    let mut errors_noted = false;
    // Global budgets shared across every root (#225 B03): total entries
    // scanned and total SKILL.md candidates. Per-root caps alone let
    // the env-supplied root list multiply work and retained state
    // without bound.
    let mut entries_left = MAX_TOTAL_DISCOVER_ENTRIES;
    let mut candidates_left = MAX_SKILL_CANDIDATES;

    for requested in roots {
        if capacity_truncated || entries_left == 0 || candidates_left == 0 {
            if !capacity_truncated {
                capacity_truncated = true;
                push_discovery_error(
                    &mut errors,
                    "discovery truncated: global entry or candidate budget exhausted".to_string(),
                    &mut errors_noted,
                );
            }
            break;
        }
        let Ok(root) = std::fs::canonicalize(requested) else {
            push_discovery_error(
                &mut errors,
                format!("root not found or unreadable: {}", requested.display()),
                &mut errors_noted,
            );
            continue;
        };
        if !root.is_dir() {
            // An existing non-directory root (e.g. a file path) is an
            // operator/agent error worth surfacing, not a silent skip.
            push_discovery_error(
                &mut errors,
                format!("root is not a directory: {}", requested.display()),
                &mut errors_noted,
            );
            continue;
        }
        if !seen_roots.insert(root.clone()) {
            continue;
        }
        // Stream entries WITHOUT sort_by_file_name: it buffers every
        // dentry of a directory before yielding anything, so a hostile
        // corpus with millions of entries in one directory would
        // exhaust memory before the entry cap applies (#144 r19).
        // Collect bounded SKILL.md candidates instead, then sort them
        // component-wise — the same order sort_by_file_name's DFS
        // traversal produces — so discovery order stays deterministic.
        // Under entry-cap pressure the candidate SET is filesystem
        // order dependent (accepted residual: corpora beyond the cap
        // are already reported truncated).
        //
        // max_depth bounds open fds to ~MAX_DISCOVER_DEPTH+1 (#225 B02)
        // — see the const's comment for why a small max_open is the
        // wrong trade (its close() drains whole directories into
        // memory). max_open stays usize::MAX: with the stack bounded
        // by depth it can never trigger, so no directory is ever
        // close()-drained.
        let mut entries = WalkDir::new(&root)
            .follow_links(false)
            .max_depth(MAX_DISCOVER_DEPTH)
            .max_open(usize::MAX)
            .into_iter();
        let mut consumed = 0usize;
        let mut walk_errors = 0usize;
        let mut depth_limited = false;
        let mut candidates: Vec<PathBuf> = Vec::new();
        let root_budget = MAX_DISCOVER_ENTRIES.min(entries_left);
        for entry in entries.by_ref().take(root_budget) {
            consumed += 1;
            entries_left -= 1;
            // Surface IO errors (permission-denied dirs etc.) instead of
            // silently reporting a partial scan as clean.
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    if walk_errors < 100 {
                        push_discovery_error(
                            &mut errors,
                            format!(
                                "{}: walk error at {}: {err}",
                                root.display(),
                                err.path()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default()
                            ),
                            &mut errors_noted,
                        );
                    }
                    walk_errors += 1;
                    continue;
                }
            };
            if entry.file_type().is_symlink() || entry.file_name() != "SKILL.md" {
                // A directory AT the depth bound is yielded but never
                // descended — any SKILL.md below it is unreachable.
                // Record it once so the truncation is explicit (#225 B02).
                if entry.depth() >= MAX_DISCOVER_DEPTH && entry.file_type().is_dir() {
                    depth_limited = true;
                }
                continue;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Contained/overlapping roots can surface the same SKILL.md
            // twice; count each file once. The canonical path must
            // still resolve INSIDE the canonical root: an ancestor
            // swapped for a symlink between walk and canonicalize
            // would otherwise admit an out-of-scope file into the
            // corpus (#225 D01 — the fd-pinned open in parser.rs is
            // the second layer).
            let Ok(canonical) = std::fs::canonicalize(path) else {
                continue;
            };
            if !canonical.starts_with(&root) {
                continue;
            }
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                canonical.hash(&mut h);
                h.finish()
            };
            if !seen_files.insert(key) {
                continue;
            }
            candidates.push(path.to_path_buf());
            candidates_left -= 1;
            if candidates_left == 0 {
                // Parse can consume at most 2*MAX_SKILLS candidates
                // GLOBALLY (skill + error caps), so further SKILL.md
                // files are unreachable — stop the traversal instead
                // of scanning to the entry cap (#144 r19 CR, #225 B03).
                push_discovery_error(
                    &mut errors,
                    format!(
                        "{}: SKILL.md candidate limit {} reached",
                        root.display(),
                        MAX_SKILL_CANDIDATES
                    ),
                    &mut errors_noted,
                );
                break;
            }
        }
        if depth_limited {
            push_discovery_error(
                &mut errors,
                format!(
                    "{}: directory depth limit {} reached; deeper entries not scanned",
                    root.display(),
                    MAX_DISCOVER_DEPTH
                ),
                &mut errors_noted,
            );
        }
        // Component-wise compare reproduces per-directory name sort +
        // DFS descent: "a" < "a-b" puts a/'s whole subtree first,
        // matching sort_by_file_name traversal order for SKILL.md
        // leaves (#144 r19).
        candidates.sort_by(|a, b| {
            a.components()
                .map(|c| c.as_os_str())
                .cmp(b.components().map(|c| c.as_os_str()))
        });
        for path in &candidates {
            let path = path.as_path();
            let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if skills.len() >= MAX_SKILLS || total_bytes + file_bytes > MAX_SKILL_BYTES {
                capacity_truncated = true;
                break;
            }
            match parse_skill(path, &root) {
                Ok(skill) => {
                    // Charge retained bytes, not pre-read metadata:
                    // from_utf8_lossy can expand invalid UTF-8 ~3x and
                    // a file that grows between metadata() and the
                    // capped read is undercounted by file_bytes.
                    let retained = (skill.body_text.len()
                        + skill.name.len()
                        + skill.description.len()
                        + skill.keywords.iter().map(|k| k.len()).sum::<usize>()
                        + skill
                            .metadata
                            .iter()
                            .map(|(k, v)| k.len() + v.len())
                            .sum::<usize>()
                        + skill
                            .links
                            .iter()
                            .map(|l| l.target.len() + 64)
                            .sum::<usize>()
                        // digest (64 B), parse_error text, root path and
                        // SkillLink struct are small but retained;
                        // charge a flat allowance plus their real lens.
                        + skill.digest.len()
                        + skill.parse_error.as_deref().map_or(0, str::len)
                        + skill.root.as_os_str().len()
                        + skill.path.as_os_str().len()
                        // Field lens count CONTENT bytes only. The
                        // structural overhead they miss — HashMap slots,
                        // String headers, Vec element+capacity, the
                        // SkillLink struct itself — is charged per
                        // entry/capacity so a max-size metadata map or
                        // link list cannot slip ~10-16KB/skill past the
                        // 512 MiB cap uncounted (#144 R25).
                        + skill.metadata.len() * 128
                        + skill.keywords.capacity() * 32
                        + skill.links.capacity() * 112
                        + 256) as u64;
                    let charge = retained.max(file_bytes);
                    // #225 B01: enforce the retention budget BEFORE
                    // push — the pre-parse check used file_bytes, but
                    // retained (UTF-8 expansion, struct overhead) can
                    // exceed it, so the LAST accepted skill could push
                    // total_bytes past MAX_SKILL_BYTES. A skill that
                    // parses but does not fit is dropped with the same
                    // explicit truncation notice.
                    if total_bytes.saturating_add(charge) > MAX_SKILL_BYTES {
                        capacity_truncated = true;
                        break;
                    }
                    total_bytes = total_bytes.saturating_add(charge);
                    skills.push(skill);
                }
                Err(err) => {
                    // Parse failures do not consume the skill/byte caps,
                    // so bound them separately: an unreadable corpus
                    // must not grow `errors`/`seen_files` without limit
                    // in a long-lived MCP server.
                    errored += 1;
                    push_discovery_error(&mut errors, err, &mut errors_noted);
                    if errored >= MAX_SKILLS {
                        capacity_truncated = true;
                        break;
                    }
                }
            }
        }
        // `consumed` counts every iterator item including walk errors;
        // truncation is about consumed budget, not successfully read
        // entries. The budget is min(per-root, remaining global), so a
        // global-budget stop is reported here too — otherwise the last
        // root could exhaust it silently (#225 B03).
        if consumed >= root_budget && entries.next().is_some() {
            push_discovery_error(
                &mut errors,
                format!(
                    "{}: entry scan truncated at {} entries",
                    root.display(),
                    root_budget
                ),
                &mut errors_noted,
            );
        }
    }

    if capacity_truncated {
        push_discovery_error(
            &mut errors,
            format!(
                "discovery truncated: skill limit {} or total size limit {} MiB reached",
                MAX_SKILLS,
                MAX_SKILL_BYTES / (1024 * 1024)
            ),
            &mut errors_noted,
        );
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
