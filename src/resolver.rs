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

    let mut backlinks: HashMap<usize, Vec<usize>> = HashMap::new();
    for (source, skill) in skills.iter().enumerate() {
        for link in &skill.links {
            if let Some(name) = &link.resolved_name
                && let Some(targets) = by_name.get(name)
                && targets.len() == 1
            {
                backlinks.entry(targets[0]).or_default().push(source);
            }
        }
    }

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
    if let Some(indexes) = by_name.get(&link.target)
        && indexes.len() == 1
    {
        link.resolved_name = Some(link.target.clone());
    }
}

fn add_mentions(skills: &mut [Skill], index: usize, by_name: &HashMap<String, Vec<usize>>) {
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

    let mut mentions = Vec::new();
    for token in tokens {
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
