use crate::model::{LinkKind, Skill, SkillLink};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct Graph {
    pub skills: Vec<Skill>,
    pub by_name: HashMap<String, Vec<usize>>,
    pub by_path: HashMap<PathBuf, usize>,
    /// target index -> number of distinct source skills linking it.
    /// Only the count is consumed (search.rs backlink reason), so the
    /// per-target source list is never materialized: name-resolved
    /// links would otherwise fan out O(sources × same-name copies)
    /// pairs — ~100M inserts / >1 GB on a max corpus (#144 r18).
    pub backlinks: HashMap<usize, usize>,
    pub ambiguous_names: Vec<String>,
}

/// Resolver-added retention (resolved_name/resolved_path clones and
/// derived Mention links) is NOT covered by discovery's per-link
/// `target.len() + 64` charge: a resolved_path carries the full parent
/// path plus target, and each skill can gain 64 mention links after
/// parsing. On a max corpus that is gigabytes of unaccounted memory,
/// so resolution draws from this global budget instead (#144 r19).
/// Exhaustion leaves links unresolved — a graceful degradation that
/// only drops backlink/requires evidence on hostile corpora.
const MAX_RESOLVE_BYTES: u64 = 128 * 1024 * 1024;

pub fn build(discovery: crate::discovery::Discovery) -> Graph {
    let mut skills = discovery.skills;
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_path = HashMap::new();
    let mut resolve_budget = MAX_RESOLVE_BYTES;

    for (index, skill) in skills.iter().enumerate() {
        by_name.entry(skill.name.clone()).or_default().push(index);
        by_path.insert(normalize_path(&skill.path), index);
    }

    let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    let parents: Vec<PathBuf> = skills
        .iter()
        .filter_map(|s| s.path.parent().map(Path::to_path_buf))
        .collect();
    // `parents[index]` below assumes 1:1 alignment with `skills` — true
    // today because every discovered path ends in SKILL.md under a
    // canonicalized root, but `filter_map` would silently misalign the
    // index→parent mapping if a parentless path ever slipped in.
    debug_assert_eq!(parents.len(), skills.len());

    for index in 0..skills.len() {
        let mut links = skills[index].links.clone();
        for link in &mut links {
            match link.kind {
                LinkKind::Markdown => {
                    resolve_markdown(&parents[index], link, &by_path, &names, &mut resolve_budget);
                }
                LinkKind::Requires => resolve_requires(link, &by_name, &mut resolve_budget),
                _ => {}
            }
        }
        skills[index].links = links;
        add_mentions(&mut skills, index, &by_name, &mut resolve_budget);
    }

    // Backlinks attach to resolved target copies, not just the first one.
    // A link with a resolved path identifies one concrete copy; a
    // name-resolved link (Requires/Mention) references the skill *name*,
    // so every installed copy of that name is a valid resolution and each
    // carries the backlink. Sources are deduplicated per target: a skill
    // referencing the same copy through several link kinds (e.g. a
    // Markdown link plus Requires) counts once.
    //
    // Name-resolved links are aggregated per NAME, not per copy: every
    // copy of a name shares the same name-source set, so materializing
    // (target, source) pairs would cost O(links × same-name copies) —
    // ~100M pairs on a 10K-skill corpus of duplicate installs (#144
    // r18). Path-resolved links stay per-target. The final count per
    // target is |name_sources[name] ∪ path_sources[target]|, computed
    // as name-set size plus path-set members absent from the name set —
    // O(links + targets × path-set size), independent of copy count.
    let mut name_sources: HashMap<&str, HashSet<usize>> = HashMap::new();
    let mut path_sources: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (source, skill) in skills.iter().enumerate() {
        for link in &skill.links {
            if let Some(path) = &link.resolved_path
                && let Some(&target) = by_path.get(path)
            {
                path_sources.entry(target).or_default().insert(source);
                continue;
            }
            if let Some(name) = &link.resolved_name {
                name_sources
                    .entry(name.as_str())
                    .or_default()
                    .insert(source);
            }
        }
    }
    let backlinks: HashMap<usize, usize> = skills
        .iter()
        .enumerate()
        .filter_map(|(index, skill)| {
            let name_set = name_sources.get(skill.name.as_str());
            let base = name_set.map_or(0, HashSet::len);
            let extra = path_sources.get(&index).map_or(0, |set| {
                set.iter()
                    .filter(|s| name_set.is_none_or(|ns| !ns.contains(s)))
                    .count()
            });
            (base + extra > 0).then_some((index, base + extra))
        })
        .collect();

    let mut ambiguous_names: Vec<String> = by_name
        .iter()
        .filter(|(_, indexes)| indexes.len() > 1)
        .map(|(name, _)| name.clone())
        .collect();
    // HashMap iteration order is random; keep the public field
    // deterministic for any consumer that iterates it (#144 r15).
    ambiguous_names.sort_unstable();

    Graph {
        skills,
        by_name,
        by_path,
        backlinks,
        ambiguous_names,
    }
}

fn resolve_markdown(
    parent: &Path,
    link: &mut SkillLink,
    by_path: &HashMap<PathBuf, usize>,
    names: &[String],
    budget: &mut u64,
) {
    let joined = normalize_path(&parent.join(&link.target));
    let candidates = if link.target.ends_with(".md") {
        vec![joined.clone()]
    } else {
        vec![joined.join("SKILL.md"), joined]
    };

    for candidate in candidates {
        if let Some(index) = by_path.get(&candidate) {
            // Charge the retained clones: resolved_name is a full name
            // copy and resolved_path the whole normalized path — both
            // larger than discovery's flat per-link allowance (#144 r19).
            let charge = candidate.as_os_str().len() as u64 + names[*index].len() as u64;
            if charge > *budget {
                return;
            }
            *budget -= charge;
            link.resolved_name = Some(names[*index].clone());
            link.resolved_path = Some(candidate);
            return;
        }
        if link.target.ends_with(".md") && candidate.starts_with(parent) {
            let charge = candidate.as_os_str().len() as u64;
            if charge > *budget {
                return;
            }
            *budget -= charge;
            link.resolved_path = Some(candidate);
            return;
        }
    }
}

fn resolve_requires(link: &mut SkillLink, by_name: &HashMap<String, Vec<usize>>, budget: &mut u64) {
    // Resolution is to the skill *name*: duplicate installs across roots share
    // the name, so the link resolves even when copies are ambiguous. Consumers
    // decide how to handle multiplicity.
    if by_name.contains_key(&link.target) {
        let charge = link.target.len() as u64;
        if charge > *budget {
            return;
        }
        *budget -= charge;
        link.resolved_name = Some(link.target.clone());
    }
}

/// Mentions are derived evidence, not authored links: the parser's
/// MAX_LINKS budget does not cover them, so cap per skill — a body that
/// names many skills must not create unbounded link/backlink blowup.
/// Deterministic: tokens are visited in sorted order.
const MAX_MENTIONS_PER_SKILL: usize = 64;

fn add_mentions(
    skills: &mut [Skill],
    index: usize,
    by_name: &HashMap<String, Vec<usize>>,
    budget: &mut u64,
) {
    if skills[index].parse_error.is_some() {
        return;
    }
    // Tokenize in an immutable borrow scope so the ~1 MiB body_text is
    // not cloned per skill — the mutable links borrow comes after.
    let mut tokens = HashSet::new();
    let mut current = String::new();
    for ch in skills[index].body_text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            current.push(ch.to_ascii_lowercase());
        } else {
            if current.len() >= 3 {
                tokens.insert(current.clone());
            }
            current.clear();
        }
    }

    let self_name = skills[index].name.clone();
    let mut tokens: Vec<String> = tokens.into_iter().collect();
    tokens.sort_unstable();
    let mut mentions = Vec::new();
    for token in tokens.into_iter().take(MAX_MENTIONS_PER_SKILL) {
        if token == self_name {
            continue;
        }
        if let Some(indexes) = by_name.get(&token)
            && indexes.len() == 1
        {
            // Each mention retains target + resolved_name clones plus
            // the SkillLink itself — derived links the parser's budget
            // never saw, so they draw from the shared resolve budget
            // (#144 r19). Sorted token order keeps truncation
            // deterministic.
            let charge = token.len() as u64 * 2 + 384;
            if charge > *budget {
                break;
            }
            *budget -= charge;
            mentions.push(SkillLink {
                target: token.clone(),
                kind: LinkKind::Mention,
                resolved_name: Some(token.clone()),
                resolved_path: None,
            });
        }
    }
    skills[index].links.extend(mentions);
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
