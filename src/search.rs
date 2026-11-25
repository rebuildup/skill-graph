use crate::model::{LinkKind, Recommendation};
use crate::resolver::Graph;
use std::collections::HashMap;

struct Evidence {
    direct: f64,
    weak: f64,
    necessity: f64,
    query_hits: usize,
    context_hits: usize,
    reasons: Vec<String>,
}

pub fn recommend(graph: &Graph, query: &str, context: &str, max: usize) -> Vec<Recommendation> {
    let query = query.trim().to_lowercase();
    let context = context.trim().to_lowercase();
    if query.is_empty() && context.is_empty() {
        return Vec::new();
    }

    let mut infos: Vec<Option<Evidence>> = Vec::new();
    infos.resize_with(graph.skills.len(), || None);

    for (index, skill) in graph.skills.iter().enumerate() {
        if skill.parse_error.is_some() {
            continue;
        }
        let mut direct_reasons = Vec::new();
        let mut weak_reasons = Vec::new();

        // Direct applicability evidence: the skill's own name/description.
        let direct = field_score(&query, &skill.name, 20.0, "name", &mut direct_reasons)
            + field_score(
                &query,
                &skill.description,
                8.0,
                "description",
                &mut direct_reasons,
            )
            + field_score(
                &context,
                &skill.description,
                4.0,
                "context+description",
                &mut direct_reasons,
            );

        // Weak relatedness evidence: body text and link targets. This can rank
        // an admitted skill but cannot admit one by itself.
        let mut weak = field_score(&query, &skill.body_text, 2.0, "body", &mut weak_reasons)
            + field_score(
                &context,
                &skill.body_text,
                1.0,
                "context+body",
                &mut weak_reasons,
            );
        for link in &skill.links {
            match link.kind {
                LinkKind::Requires | LinkKind::Markdown if matches_query(&query, &link.target) => {
                    weak += if link.kind == LinkKind::Requires {
                        3.0
                    } else {
                        1.0
                    };
                    weak_reasons.push(format!("{:?} link target: {}", link.kind, link.target));
                }
                LinkKind::Mention if matches_query(&query, &link.target) => {
                    weak += 1.0;
                    weak_reasons.push(format!("mention: {}", link.target));
                }
                _ => {}
            }
        }

        let mut reasons = direct_reasons;
        reasons.extend(weak_reasons);
        reasons.sort();
        reasons.dedup();

        let name_lower = skill.name.to_lowercase();
        let description_lower = skill.description.to_lowercase();
        let query_hits = query
            .split_whitespace()
            .filter(|term| term.len() >= 2)
            .filter(|term| name_lower.contains(*term) || description_lower.contains(*term))
            .count();
        let context_hits = context
            .split_whitespace()
            .filter(|term| term.len() >= 2)
            .filter(|term| description_lower.contains(*term))
            .count();

        infos[index] = Some(Evidence {
            direct,
            weak,
            necessity: 0.0,
            query_hits,
            context_hits,
            reasons,
        });
    }

    // Admission is structural, not score-based:
    // - query relevance (name/description) or at least two independent context
    //   description matches admits a skill;
    // - an explicit `requires` declaration admits its dependency when the
    //   declaring skill is admitted (necessity, one hop only);
    // - Markdown links, mentions, body text, and backlinks never admit a skill.
    let mut admitted: HashMap<usize, Vec<String>> = HashMap::new();
    let mut seeds = Vec::new();
    for (index, info) in infos.iter().enumerate() {
        if info
            .as_ref()
            .is_some_and(|e| e.query_hits > 0 || e.context_hits >= 2)
        {
            admitted.insert(index, Vec::new());
            seeds.push(index);
        }
    }

    for seed in &seeds {
        for link in &graph.skills[*seed].links {
            if link.kind != LinkKind::Requires {
                continue;
            }
            let Some(name) = &link.resolved_name else {
                continue;
            };
            let Some(targets) = graph.by_name.get(name) else {
                continue;
            };
            if targets.len() != 1 {
                continue;
            }
            let target = targets[0];
            if graph.skills[target].parse_error.is_some() {
                continue;
            }
            let is_seed = infos[target]
                .as_ref()
                .is_some_and(|e| e.query_hits > 0 || e.context_hits >= 2);
            let entry = admitted.entry(target).or_default();
            entry.push(format!("required by {}", graph.skills[*seed].name));
            if !is_seed && let Some(info) = infos[target].as_mut() {
                info.necessity += 2.0;
            }
        }
    }

    let mut out: Vec<Recommendation> = infos
        .iter_mut()
        .enumerate()
        .filter_map(|(index, info)| {
            let info = info.as_mut()?;
            let mut extra_reasons = admitted.get(&index)?.clone();
            let score = info.direct + info.weak + info.necessity;
            if score <= 0.0 {
                return None;
            }
            let mut reasons = std::mem::take(&mut info.reasons);
            reasons.append(&mut extra_reasons);
            if let Some(backlinks) = graph.backlinks.get(&index)
                && !backlinks.is_empty()
            {
                reasons.push(format!("backlinks: {}", backlinks.len()));
            }
            reasons.sort();
            reasons.dedup();
            Some(Recommendation {
                skill: graph.skills[index].summary(),
                score,
                reasons,
                observation: "filesystem_only".to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(max);
    out
}

fn field_score(
    query: &str,
    field: &str,
    weight: f64,
    label: &str,
    reasons: &mut Vec<String>,
) -> f64 {
    if query.is_empty() || field.is_empty() {
        return 0.0;
    }
    let field_lower = field.to_lowercase();
    let mut score = 0.0;
    for term in query.split_whitespace() {
        if term.len() < 2 {
            continue;
        }
        if field_lower.contains(term) {
            score += weight;
            reasons.push(format!("{label}: {term}"));
        }
    }
    if !query.contains(char::is_whitespace) && field_lower.contains(query) {
        score += weight * 2.0;
        reasons.push(format!("{label}: {query}"));
    }
    score
}

fn matches_query(query: &str, target: &str) -> bool {
    !query.is_empty() && target.to_lowercase().contains(query)
}
