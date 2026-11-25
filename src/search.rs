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
    // Match text for description-level evidence: the description plus any
    // declared `keywords` (e.g. translations or aliases). Keywords are
    // opt-in frontmatter, so corpus df stays honest.
    let skill_descriptions: Vec<String> = graph
        .skills
        .iter()
        .map(|skill| {
            let mut text = skill.description.to_lowercase();
            for keyword in &skill.keywords {
                text.push(' ');
                text.push_str(&keyword.to_lowercase());
            }
            text
        })
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

    for (index, skill) in graph.skills.iter().enumerate() {
        if skill.parse_error.is_some() {
            continue;
        }
        let name_lower = &skill_names[index];
        let description_lower = skill_descriptions[index].as_str();
        let mut direct_reasons = Vec::new();
        let mut weak_reasons = Vec::new();

        // Direct applicability evidence: the skill's own name/description.
        let direct = field_score(
            &query,
            &query_terms,
            &skill.name,
            20.0,
            "name",
            &mut direct_reasons,
        ) + field_score(
            &query,
            &query_terms,
            skill_descriptions[index].as_str(),
            8.0,
            "description",
            &mut direct_reasons,
        ) + field_score(
            &context,
            &context_terms,
            skill_descriptions[index].as_str(),
            4.0,
            "context+description",
            &mut direct_reasons,
        );

        // Weak relatedness evidence: body text and link targets. This can rank
        // an admitted skill but cannot admit one by itself.
        let mut weak = field_score(
            &query,
            &query_terms,
            &skill.body_text,
            2.0,
            "body",
            &mut weak_reasons,
        ) + field_score(
            &context,
            &context_terms,
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
            let Some(targets) = graph.by_name.get(name) else {
                continue;
            };
            for &target in targets {
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

    // Recommendation identity is the skill name, but same-name copies are not
    // silently conflated: the highest-scoring copy represents the name, and a
    // name shared by distinct digests is reported as a collision in reasons.
    for rec in &mut out {
        if let Some(indexes) = graph.by_name.get(&rec.skill.name) {
            let distinct: HashSet<&str> = indexes
                .iter()
                .filter(|&&i| graph.skills[i].parse_error.is_none())
                .map(|&i| graph.skills[i].digest.as_str())
                .collect();
            if distinct.len() > 1 {
                rec.reasons.push(format!(
                    "name collision: {} distinct versions installed",
                    distinct.len()
                ));
                rec.reasons.sort();
                rec.reasons.dedup();
            }
        }
    }
    let mut seen_names = HashSet::new();
    out.retain(|rec| seen_names.insert(rec.skill.name.clone()));
    out.truncate(max);
    out
}

fn directly_admitted(info: &Evidence) -> bool {
    // The skill name is the skill's identity: two or more query terms inside
    // one name is specific regardless of corpus-wide document frequency.
    // A single name hit stays conservative (low df, meaningful length).
    let multiple_name_hits = info.name_hits.len() >= 2;
    let unique_name_hit =
        info.name_hits.len() == 1 && info.name_hits[0].1 == 1 && info.name_hits[0].0.len() >= 4;
    // A name hit plus independent description evidence admits: the name anchors
    // identity while the description confirms topical fit. This keeps high-df
    // description terms from admitting alone but lets e.g. "PR review" reach
    // `code-review` on real-size corpora.
    let name_plus_description = !info.name_hits.is_empty() && info.description_hits.len() >= 2;
    let description_admitted =
        info.description_hits.len() >= 2 && info.description_hits.iter().any(|(_, df)| *df == 1);
    let context_admitted = info.context_hits.len() >= 2;
    multiple_name_hits
        || unique_name_hit
        || name_plus_description
        || description_admitted
        || context_admitted
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
        // df counts distinct skill names: a term matches a name when any
        // installed copy of that name contains it, so duplicate installs do
        // not inflate frequency.
        let count = candidates
            .iter()
            .filter(|&&index| {
                let Some(copies) = graph.by_name.get(&graph.skills[index].name) else {
                    return false;
                };
                copies.iter().any(|&copy| {
                    graph.skills[copy].parse_error.is_none()
                        && (skill_names[copy].contains(term)
                            || skill_descriptions[copy].contains(term))
                })
            })
            .count();
        frequencies.insert(term.to_string(), count);
    }
    frequencies
}

fn field_score(
    query: &str,
    query_terms: &[String],
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
    for term in query_terms {
        if field_lower.contains(term.as_str()) {
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

// English function words and generic prompt filler carry no skill-selection
// signal; matching them against descriptions produces noise admissions on
// non-task prompts (observed in S02 live verification, issue #46). Terms are
// compared after lowercasing, so the list is lowercase.
const STOPWORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "after",
    "again",
    "all",
    "also",
    "am",
    "an",
    "and",
    "answer",
    "any",
    "are",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "doing",
    "down",
    "during",
    "each",
    "few",
    "for",
    "from",
    "further",
    "get",
    "got",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "itself",
    "just",
    "keep",
    "keeps",
    "let",
    "like",
    "me",
    "more",
    "most",
    "my",
    "myself",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "please",
    "same",
    "she",
    "should",
    "so",
    "some",
    "such",
    "than",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "why",
    "will",
    "with",
    "within",
    "without",
    "would",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut ascii_run = String::new();
    let mut cjk_run = String::new();
    let mut flush = |ascii: &mut String, cjk: &mut String| {
        if ascii.chars().count() >= 2 {
            let lower = ascii.to_lowercase();
            if !STOPWORDS.contains(&lower.as_str()) {
                out.push(lower);
            }
        }
        ascii.clear();
        // CJK has no whitespace and no case: emit character bigrams so
        // substring matching works against Japanese descriptions (#61).
        let chars: Vec<char> = cjk.chars().collect();
        for window in chars.windows(2) {
            out.push(window.iter().collect());
        }
        cjk.clear();
    };
    for raw in text.split_whitespace() {
        // Strip surrounding punctuation so tokens like `->` or `"word,`
        // cannot match description text, then split script runs: ASCII
        // words stay whole, CJK runs become bigrams. Mixed tokens like
        // "Herdrでworkerを並列に" yield both "herdr"/"worker" and CJK
        // bigrams.
        for c in raw.trim_matches(|c: char| !c.is_alphanumeric()).chars() {
            if c.is_ascii_alphanumeric() {
                if !cjk_run.is_empty() {
                    flush(&mut ascii_run, &mut cjk_run);
                }
                ascii_run.push(c);
            } else if is_cjk(c) {
                if !ascii_run.is_empty() {
                    flush(&mut ascii_run, &mut cjk_run);
                }
                cjk_run.push(c);
            } else {
                flush(&mut ascii_run, &mut cjk_run);
            }
        }
        flush(&mut ascii_run, &mut cjk_run);
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0x1100..=0x11FF
    )
}

#[cfg(test)]
mod tests {
    use super::terms;

    #[test]
    fn cjk_query_produces_bigrams() {
        let t = terms("不具合報告をトリアージして");
        eprintln!("terms={t:?}");
        assert!(t.iter().any(|x| x == "不具"), "{t:?}");
        assert!(t.iter().any(|x| x == "報告"), "{t:?}");
    }
}
