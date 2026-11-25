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

pub fn discover(roots: &[PathBuf]) -> Discovery {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut seen_roots = HashSet::new();

    for requested in roots {
        let Ok(root) = std::fs::canonicalize(requested) else {
            continue;
        };
        if !root.is_dir() || !seen_roots.insert(root.clone()) {
            continue;
        }
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if entry.file_type().is_symlink() || entry.file_name() != "SKILL.md" {
                continue;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            match parse_skill(path, &root) {
                Ok(skill) => skills.push(skill),
                Err(err) => errors.push(err),
            }
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
