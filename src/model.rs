use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub path: PathBuf,
    pub root: PathBuf,
    pub digest: String,
    pub metadata: HashMap<String, String>,
    pub links: Vec<SkillLink>,
    pub body_text: String,
    pub parse_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillLink {
    pub target: String,
    pub kind: LinkKind,
    pub resolved_name: Option<String>,
    pub resolved_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Markdown,
    Requires,
    Mention,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    ParseError,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub digest: String,
    pub availability: Availability,
}

impl Skill {
    pub fn summary(&self) -> SkillSummary {
        SkillSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            path: self.path.clone(),
            root: self.root.clone(),
            digest: self.digest.clone(),
            availability: if self.parse_error.is_some() {
                Availability::ParseError
            } else {
                Availability::Available
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Recommendation {
    pub skill: SkillSummary,
    pub score: f64,
    pub reasons: Vec<String>,
    pub observation: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryOutput {
    pub skills: Vec<SkillSummary>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolveResult {
    Name {
        ambiguous: bool,
        skills: Vec<SkillSummary>,
    },
    Path {
        skill: SkillSummary,
    },
    Unresolved {
        target: String,
    },
}
