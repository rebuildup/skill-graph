use crate::model::{LinkKind, Recommendation};
use crate::resolver::Graph;
use std::collections::{HashMap, HashSet};

struct Evidence {
    direct: f64,
    weak: f64,
    necessity: f64,
    name_hits: Vec<(String, usize)>,
    description_hits: Vec<(String, usize)>,
    context_hits: Vec<(String, usize)>,
    reasons: Vec<String>,
}

pub fn recommend(graph: &Graph, query: &str, context: &str, max: usize) -> Vec<Recommendation> {
    let query = query.trim().to_lowercase();
    let context = context.trim().to_lowercase();
    if query.is_empty() && context.is_empty() {
        return Vec::new();
    }

    let query_terms = terms(&query);
    let context_terms = terms(&context);
    let skill_names: Vec<String> = graph
        .skills
        .iter()
        .map(|skill| skill.name.to_lowercase())
        .collect();
    let skill_descriptions: Vec<String> = graph
        .skills
        .iter()
        .map(|skill| skill.description.to_lowercase())
        .collect();
    // Recommendation identity is the skill name: duplicate installs of the
    // same skill across roots collapse to one candidate. The representative is
    // the first copy without a parse error (discovery order is deterministic).
    let mut rep_of_name: HashMap<&str, usize> = HashMap::new();
    for (index, skill) in graph.skills.iter().enumerate() {
        let replace = match rep_of_name.get(skill.name.as_str()) {
            None => true,
            Some(&prev) => graph.skills[prev].parse_error.is_some() && skill.parse_error.is_none(),
        };
        if replace {
            rep_of_name.insert(skill.name.as_str(), index);
        }
    }
    let mut candidates: Vec<usize> = rep_of_name.values().copied().collect();
    candidates.sort_unstable();

    let document_frequency = document_frequency(
        graph,
        &candidates,
        &skill_names,
        &skill_descriptions,
        query_terms.iter().chain(context_terms.iter()),
    );

    let mut infos: Vec<Option<Evidence>> = Vec::new();
    infos.resize_with(graph.skills.len(), || None);

    for &index in &candidates {
        let skill = &graph.skills[index];
        if skill.parse_error.is_some() {
            continue;
        }
        let name_lower = &skill_names[index];
        let description_lower = &skill_descriptions[index];
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

        let name_hits = query_terms
            .iter()
            .filter(|term| name_lower.contains(term.as_str()))
            .map(|term| {
                (
                    term.clone(),
                    *document_frequency.get(term).unwrap_or(&usize::MAX),
                )
            })
            .collect::<Vec<_>>();
        let description_hits = query_terms
            .iter()
            .filter(|term| description_lower.contains(term.as_str()))
            .map(|term| {
                (
                    term.clone(),
                    *document_frequency.get(term).unwrap_or(&usize::MAX),
                )
            })
            .collect::<Vec<_>>();
        let context_hits = context_terms
            .iter()
            .filter(|term| description_lower.contains(term.as_str()))
            .map(|term| {
                (
                    term.clone(),
                    *document_frequency.get(term).unwrap_or(&usize::MAX),
                )
            })
            .collect::<Vec<_>>();

        infos[index] = Some(Evidence {
            direct,
            weak,
            necessity: 0.0,
            name_hits,
            description_hits,
            context_hits,
            reasons,
        });
    }

    // Admission is structural, not score-based:
    // - a name hit admits when it is specific enough (multiple name terms or a
    //   single low-df term of meaningful length);
    // - description hits admit when at least two independent query terms match
    //   and at least one is low-df, so generic words in broad descriptions do
    //   not admit by themselves;
    // - at least two independent context-term hits in the description admit;
    // - an explicit `requires` declaration admits its dependency when the
    //   declaring skill is admitted (necessity, one hop only);
    // - Markdown links, mentions, body text, and backlinks never admit.
    let mut admitted: HashMap<usize, Vec<String>> = HashMap::new();
    let mut seeds = Vec::new();
    for (index, info) in infos.iter().enumerate() {
        if info.as_ref().is_some_and(directly_admitted) {
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
            let Some(&target) = rep_of_name.get(name.as_str()) else {
                continue;
            };
            if graph.skills[target].parse_error.is_some() {
                continue;
            }
            let is_seed = infos[target].as_ref().is_some_and(directly_admitted);
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

fn directly_admitted(info: &Evidence) -> bool {
    let unique_name_hit =
        info.name_hits.len() == 1 && info.name_hits[0].1 == 1 && info.name_hits[0].0.len() >= 4;
    let multiple_name_hits =
        info.name_hits.len() >= 2 && info.name_hits.iter().any(|(_, df)| *df == 1);
    let description_admitted =
        info.description_hits.len() >= 2 && info.description_hits.iter().any(|(_, df)| *df == 1);
    let context_admitted = info.context_hits.len() >= 2;
    unique_name_hit || multiple_name_hits || description_admitted || context_admitted
}

fn document_frequency<'a>(
    graph: &Graph,
    candidates: &[usize],
    skill_names: &[String],
    skill_descriptions: &[String],
    terms: impl Iterator<Item = &'a String>,
) -> HashMap<String, usize> {
    let mut unique = HashSet::new();
    for term in terms {
        unique.insert(term.as_str());
    }
    let mut frequencies = HashMap::new();
    for term in unique {
        let count = candidates
            .iter()
            .filter(|&&index| {
                graph.skills[index].parse_error.is_none()
                    && (skill_names[index].contains(term)
                        || skill_descriptions[index].contains(term))
            })
            .count();
        frequencies.insert(term.to_string(), count);
    }
    frequencies
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

fn terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|term| term.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}
