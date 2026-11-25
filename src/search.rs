use crate::model::{LinkKind, Recommendation};
use crate::resolver::Graph;
use std::collections::HashMap;

pub fn recommend(graph: &Graph, query: &str, context: &str, max: usize) -> Vec<Recommendation> {
    let query = query.trim().to_lowercase();
    let context = context.trim().to_lowercase();
    if query.is_empty() && context.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, f64, Vec<String>)> = Vec::new();
    for (index, skill) in graph.skills.iter().enumerate() {
        if skill.parse_error.is_some() {
            continue;
        }
        let mut reasons = Vec::new();
        let mut score = 0.0;

        score += field_score(&query, &skill.name, 20.0, "name", &mut reasons);
        score += field_score(&query, &skill.description, 8.0, "description", &mut reasons);
        score += field_score(&query, &skill.body_text, 2.0, "body", &mut reasons);
        score += field_score(
            &context,
            &skill.description,
            4.0,
            "context+description",
            &mut reasons,
        );
        score += field_score(
            &context,
            &skill.body_text,
            1.0,
            "context+body",
            &mut reasons,
        );

        for link in &skill.links {
            match link.kind {
                LinkKind::Requires | LinkKind::Markdown if matches_query(&query, &link.target) => {
                    score += 3.0;
                    reasons.push(format!("{:?} link/dependency: {}", link.kind, link.target));
                }
                LinkKind::Mention if matches_query(&query, &link.target) => {
                    score += 1.0;
                    reasons.push(format!("mention: {}", link.target));
                }
                _ => {}
            }
        }

        if score > 0.0 {
            scored.push((index, score, reasons));
        }
    }

    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut boosted: HashMap<usize, f64> =
        scored.iter().take(5).map(|(i, s, _)| (*i, *s)).collect();
    for (index, _, _) in scored.iter().take(5) {
        for link in &graph.skills[*index].links {
            if let Some(name) = &link.resolved_name
                && let Some(targets) = graph.by_name.get(name)
                && targets.len() == 1
            {
                let target = targets[0];
                if graph.skills[target].parse_error.is_none() {
                    *boosted.entry(target).or_insert(0.0) += 1.5;
                }
            }
        }
    }

    let mut out: Vec<Recommendation> = scored
        .into_iter()
        .filter_map(|(index, base_score, mut reasons)| {
            let score = boosted.get(&index).copied().unwrap_or(base_score);
            if score <= 0.0 {
                return None;
            }
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
