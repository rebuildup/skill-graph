use crate::model::{LinkKind, Skill, SkillLink};
use pulldown_cmark::{Event, Parser, Tag};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
    #[serde(rename = "allowed-tools")]
    #[allow(dead_code)]
    allowed_tools: Option<String>,
}

pub fn parse_skill(path: &Path, root: &Path) -> Result<Skill, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let digest = hex_digest(&bytes);
    let text = String::from_utf8_lossy(&bytes);
    let (frontmatter, body) = split_frontmatter(&text);

    let mut parse_error = None;
    let mut meta = Frontmatter::default();
    if !frontmatter.trim().is_empty() {
        match serde_yaml::from_str::<Frontmatter>(frontmatter) {
            Ok(parsed) => meta = parsed,
            Err(err) => parse_error = Some(format!("frontmatter: {err}")),
        }
    }

    let name = meta
        .name
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let description = meta.description.unwrap_or_default();

    let mut links = Vec::new();
    for event in Parser::new(body) {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            let target = dest_url.to_string();
            let kind = if target.contains("://") || target.starts_with("mailto:") {
                LinkKind::External
            } else {
                LinkKind::Markdown
            };
            links.push(SkillLink {
                target,
                kind,
                resolved_name: None,
                resolved_path: None,
            });
        }
    }

    for key in ["requires-skills", "requires_skills"] {
        if let Some(value) = meta.metadata.get(key) {
            for item in value.split(|c: char| c == ',' || c.is_whitespace()) {
                let item = item.trim();
                if !item.is_empty() {
                    links.push(SkillLink {
                        target: item.to_string(),
                        kind: LinkKind::Requires,
                        resolved_name: None,
                        resolved_path: None,
                    });
                }
            }
        }
    }

    Ok(Skill {
        name,
        description,
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        digest,
        metadata: meta.metadata,
        links,
        body_text: body.to_string(),
        parse_error,
    })
}

fn split_frontmatter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---") else {
        return ("", text);
    };
    let Some(end) = rest.find("\n---") else {
        return ("", text);
    };
    let frontmatter = &rest[..end];
    let after = &rest[end + 4..];
    let body = after
        .strip_prefix('\n')
        .or_else(|| after.strip_prefix("\r\n"))
        .unwrap_or(after);
    (frontmatter, body)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
