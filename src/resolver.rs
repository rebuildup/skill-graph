use crate::model::{LinkKind, Skill, SkillLink};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct Graph {
    pub skills: Vec<Skill>,
    pub by_name: HashMap<String, Vec<usize>>,
    pub by_path: HashMap<PathBuf, usize>,
    pub backlinks: HashMap<usize, Vec<usize>>,
    pub ambiguous_names: Vec<String>,
}

pub fn build(discovery: crate::discovery::Discovery) -> Graph {
    let mut skills = discovery.skills;
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_path = HashMap::new();

    for (index, skill) in skills.iter().enumerate() {
        by_name.entry(skill.name.clone()).or_default().push(index);
        by_path.insert(normalize_path(&skill.path), index);
    }

    let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    let parents: Vec<PathBuf> = skills
        .iter()
        .filter_map(|s| s.path.parent().map(Path::to_path_buf))
        .collect();

    for index in 0..skills.len() {
        let mut links = skills[index].links.clone();
        for link in &mut links {
            match link.kind {
                LinkKind::Markdown => {
                    resolve_markdown(&parents[index], link, &by_path, &names);
                }
                LinkKind::Requires => resolve_requires(link, &by_name),
                _ => {}
            }
        }
        skills[index].links = links;
        add_mentions(&mut skills, index, &by_name);
    }

    // Backlinks attach to resolved target copies, not just the first one.
    // A link with a resolved path identifies one concrete copy; a
    // name-resolved link (Requires/Mention) references the skill *name*,
    // so every installed copy of that name is a valid resolution and each
    // carries the backlink. Sources are deduplicated per target: a skill
    // referencing the same copy through several link kinds (e.g. a
    // Markdown link plus Requires) counts once.
    let mut backlinks: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (source, skill) in skills.iter().enumerate() {
        for link in &skill.links {
            if let Some(path) = &link.resolved_path
                && let Some(&target) = by_path.get(path)
            {
                backlinks.entry(target).or_default().insert(source);
                continue;
            }
            if let Some(name) = &link.resolved_name
                && let Some(targets) = by_name.get(name)
            {
                for &target in targets {
                    backlinks.entry(target).or_default().insert(source);
                }
            }
        }
    }
    let backlinks = backlinks
        .into_iter()
        .map(|(target, sources)| {
            let mut sources: Vec<usize> = sources.into_iter().collect();
            sources.sort_unstable();
            (target, sources)
        })
        .collect();

    let ambiguous_names = by_name
        .iter()
        .filter(|(_, indexes)| indexes.len() > 1)
        .map(|(name, _)| name.clone())
        .collect();

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
) {
    let joined = normalize_path(&parent.join(&link.target));
    let candidates = if link.target.ends_with(".md") {
        vec![joined.clone()]
    } else {
        vec![joined.join("SKILL.md"), joined]
    };

    for candidate in candidates {
        if let Some(index) = by_path.get(&candidate) {
            link.resolved_name = Some(names[*index].clone());
            link.resolved_path = Some(candidate);
            return;
        }
        if link.target.ends_with(".md") && candidate.starts_with(parent) {
            link.resolved_path = Some(candidate);
            return;
        }
    }
}

fn resolve_requires(link: &mut SkillLink, by_name: &HashMap<String, Vec<usize>>) {
    // Resolution is to the skill *name*: duplicate installs across roots share
    // the name, so the link resolves even when copies are ambiguous. Consumers
    // decide how to handle multiplicity.
    if by_name.contains_key(&link.target) {
        link.resolved_name = Some(link.target.clone());
    }
}

/// Mentions are derived evidence, not authored links: the parser's
/// MAX_LINKS budget does not cover them, so cap per skill — a body that
/// names many skills must not create unbounded link/backlink blowup.
/// Deterministic: tokens are visited in sorted order.
const MAX_MENTIONS_PER_SKILL: usize = 64;

fn add_mentions(skills: &mut [Skill], index: usize, by_name: &HashMap<String, Vec<usize>>) {
    if skills[index].parse_error.is_some() {
        return;
    }
    let body = skills[index].body_text.clone();
    let self_name = skills[index].name.clone();
    let mut tokens = HashSet::new();
    let mut current = String::new();
    for ch in body.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            current.push(ch.to_ascii_lowercase());
        } else {
            if current.len() >= 3 {
                tokens.insert(current.clone());
            }
            current.clear();
        }
    }

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
