use crate::model::{LinkKind, Recommendation};
use crate::resolver::Graph;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

struct Evidence {
    direct: f64,
    weak: f64,
    necessity: f64,
    // Hit terms only; document frequency is attached later, computed just
    // for the union of hit terms rather than every query term.
    name_hits: Vec<String>,
    description_hits: Vec<String>,
    context_hits: Vec<String>,
    reasons: Vec<String>,
}

impl Evidence {
    fn empty() -> Self {
        Evidence {
            direct: 0.0,
            weak: 0.0,
            necessity: 0.0,
            name_hits: Vec::new(),
            description_hits: Vec::new(),
            context_hits: Vec::new(),
            reasons: Vec::new(),
        }
    }
}

// Agent-controlled input (hook stdin, --query-stdin, MCP args) must not
// turn term extraction into unbounded work: cap raw text and unique terms.
const MAX_RECOMMEND_INPUT_CHARS: usize = 32 * 1024;
const MAX_RECOMMEND_TERMS: usize = 1024;

/// Global term×byte work budget for the matching/admission phase.
/// Input caps bound query size and corpus size independently, but
/// skills × terms × field_bytes can still reach hundreds of GB of
/// substring scans before admission or the body budget is reached.
/// Past this budget later skills are skipped in deterministic index
/// order, uncomputed df terms count as non-rare, and glossary
/// expansion is skipped corpus-wide — under-admission on absurd
/// inputs beats a hang (#144 round-10).
const MAX_MATCH_TERM_WORK: usize = 1024 * 1024 * 1024;

pub fn recommend(graph: &Graph, query: &str, context: &str, max: usize) -> Vec<Recommendation> {
    let query = query
        .trim()
        .chars()
        .take(MAX_RECOMMEND_INPUT_CHARS)
        .collect::<String>()
        .to_lowercase();
    let context = context
        .trim()
        .chars()
        .take(MAX_RECOMMEND_INPUT_CHARS)
        .collect::<String>()
        .to_lowercase();
    if query.is_empty() && context.is_empty() {
        return Vec::new();
    }

    // Terms carry a provenance source: glossary expansions share the CJK
    // substring they were derived from, so admission can tell independent
    // evidence from one substring's bigram+gloss cluster (#78).
    let query_tagged = terms_tagged(&query);
    let query_tagged: Vec<(String, Rc<str>)> =
        query_tagged.into_iter().take(MAX_RECOMMEND_TERMS).collect();
    let query_terms: Vec<String> = query_tagged.iter().map(|(t, _)| t.clone()).collect();
    let query_sources = term_sources(&query_tagged);
    // Context terms carry provenance too: a single CJK run's bigram
    // cluster must not satisfy context admission by itself, same rule
    // as description admission (#144 round-10).
    let context_tagged: Vec<(String, Rc<str>)> = terms_tagged(&context)
        .into_iter()
        .take(MAX_RECOMMEND_TERMS)
        .collect();
    let context_terms: Vec<String> = context_tagged.iter().map(|(t, _)| t.clone()).collect();
    let context_sources = term_sources(&context_tagged);
    let skill_names: Vec<String> = graph
        .skills
        .iter()
        .map(|skill| skill.name.to_lowercase())
        .collect();
    // Unexpanded description text: context-level admission counts hits here
    // only, so glossary expansion cannot inflate the weakest signal (#78).
    let skill_descriptions_base: Vec<String> = graph
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
    // Glossary expansion costs ~|GLOSS| substring scans per CJK-containing
    // description; on a max-size corpus that alone can dwarf the match
    // budget. Estimate first and degrade uniformly (literal text only)
    // rather than hanging or expanding an arbitrary prefix.
    let skill_descriptions: Vec<String> = {
        // gloss_terms scans every GLOSS key for ANY non-ASCII text
        // (its fast path is is_ascii, not is_cjk) — count all of it.
        let cjk_work: usize = skill_descriptions_base
            .iter()
            .filter(|d| !d.is_ascii())
            .map(|d| d.len().saturating_mul(GLOSS.len()))
            .fold(0usize, usize::saturating_add);
        if cjk_work <= MAX_MATCH_TERM_WORK {
            skill_descriptions_base
                .iter()
                .map(|d| expand_gloss(d))
                .collect()
        } else {
            skill_descriptions_base.clone()
        }
    };
    // Body text is expanded lazily inside the admitted-only loop below:
    // expand_gloss runs ~290 substring scans per body and bodies can be
    // ~1 MiB each, so building this for every corpus skill up front would
    // dominate recommend() even though non-admitted bodies are never read.
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
    // "Rare term" bound for admission: a term stays specific evidence while
    // it appears in at most ~10% of distinct skill names, with a floor of 2.
    // The floor keeps a single same-domain addition from flipping admission
    // (the df==1 -> 2 failure reported in #170); the relative cap keeps the
    // bound meaningful as the corpus grows. Ranking is unaffected — scores
    // never consulted df, so rarer terms are not promoted, only gated.
    let df_bound = (candidates.len() / 10).max(2);

    let mut infos: Vec<Option<Evidence>> = Vec::new();
    infos.resize_with(graph.skills.len(), || None);
    let mut match_work = 0usize;
    let mut budget_noted = false;
    // field_score's whitespace-free whole-query bonus costs O(field
    // bytes) per scored field — name once for `query`, description once
    // for `query` and once for `context` — not O(query length). The
    // per-link whole-query scans are already covered by `link_bytes`.
    let query_whole = !query.contains(char::is_whitespace);
    let context_whole = !context.contains(char::is_whitespace);

    for (index, skill) in graph.skills.iter().enumerate() {
        if skill.parse_error.is_some() {
            continue;
        }
        // Charge this skill's estimated term×byte scan cost against the
        // global budget before matching anything: name+description are
        // scanned once per query term and once per context term for
        // scoring AND again for the hit lists, and every link target is
        // matched against the whole query. Skills past the budget keep
        // `infos[index] = None`, so direct admission is skipped —
        // deterministic index order, same degrade direction as the body
        // budget. Explicit `requires` propagation below can still admit
        // such a skill structurally (bounded link iteration only).
        let scan_terms = query_terms.len() + context_terms.len();
        let link_bytes: usize = skill.links.iter().map(|l| l.target.len()).sum();
        // Whole-query matches are O(field bytes) needle work per call
        // and are NOT covered by the per-term charge: field_score runs
        // a whitespace-free `term_matches(field, query)` bonus on the
        // name and description fields, plus one on the description for
        // `context`. Per-link whole-query scans cost O(target bytes) =
        // `link_bytes`, already counted.
        let desc_len = skill_descriptions[index].len();
        let whole_query_work = (if query_whole {
            skill_names[index].len() + desc_len
        } else {
            0
        })
        .saturating_add(if context_whole { desc_len } else { 0 });
        let skill_work = scan_terms
            .saturating_mul(2)
            .saturating_mul(skill_names[index].len() + skill_descriptions[index].len())
            .saturating_add(link_bytes)
            .saturating_add(whole_query_work);
        match_work = match_work.saturating_add(skill_work);
        if match_work > MAX_MATCH_TERM_WORK {
            // An exact name match is the strongest possible signal and
            // must not be silently dropped by budget exhaustion: admit
            // it with minimal evidence instead of scanning fields. Only
            // skills whose whole name equals the query qualify, so the
            // number of exemptions is bounded by duplicate-name copies.
            if !query.is_empty() && skill_names[index] == query {
                infos[index] = Some(Evidence {
                    direct: 20.0,
                    reasons: vec!["exact name match".to_string()],
                    ..Evidence::empty()
                });
            } else if !budget_noted {
                budget_noted = true;
                crate::cli::write_stderr_line(
                    "skill-graph: match work budget exhausted; remaining skills skipped",
                );
            }
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
            &skill_names[index],
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

        // Weak relatedness evidence: link targets here; body text is
        // scored only for admitted skills after admission (below) — body
        // fields can be ~1 MiB each, so scanning them for every corpus
        // skill would make recommend() quadratic in corpus size. Body
        // evidence can rank an admitted skill but never admit one.
        let mut weak = 0.0;
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
            .filter(|term| term_matches(name_lower, term))
            .cloned()
            .collect::<Vec<_>>();
        let description_hits = query_terms
            .iter()
            .filter(|term| term_matches(description_lower, term))
            .cloned()
            .collect::<Vec<_>>();
        let context_hits = context_terms
            .iter()
            .filter(|term| term_matches(&skill_descriptions_base[index], term))
            .cloned()
            .collect::<Vec<_>>();

        // Materialize Evidence only for skills carrying signal: direct or
        // weak score > 0 covers every hit (each hit field is a subset of a
        // scored field), and skills with no signal can never be admitted or
        // ranked — requires-admitted targets get a zeroed Evidence on
        // demand in the propagation loop below. This keeps per-skill hit
        // and reason vectors from being retained for the whole corpus.
        if direct > 0.0 || weak > 0.0 {
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
    }

    // Document frequency is needed only for admission decisions on terms
    // that actually hit a field; computing it for every query term would
    // scan the corpus once per term even when nothing matches.
    let hit_terms: Vec<&str> = infos
        .iter()
        .flatten()
        .flat_map(|info| {
            info.name_hits
                .iter()
                .chain(&info.description_hits)
                .chain(&info.context_hits)
                .map(|s| &**s)
        })
        .collect();
    let document_frequency = document_frequency(
        graph,
        &candidates,
        &skill_names,
        &skill_descriptions,
        hit_terms.iter().copied(),
        &mut match_work,
    );

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
        // An exact name match is the strongest possible signal and admits
        // even names too short to survive term extraction (e.g. a 1-char
        // ASCII name produces no query terms at all). Parse-error skills
        // never admit on ANY path — otherwise a malformed skill could
        // still seed requires-propagation (#144 r13).
        let exact_name = !query.is_empty()
            && graph.skills[index].parse_error.is_none()
            && skill_names[index] == query;
        if exact_name
            || info.as_ref().is_some_and(|info| {
                directly_admitted(
                    info,
                    &query_sources,
                    &context_sources,
                    &document_frequency,
                    df_bound,
                )
            })
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
            for &target in targets {
                if graph.skills[target].parse_error.is_some() {
                    continue;
                }
                let is_seed = infos[target].as_ref().is_some_and(|info| {
                    directly_admitted(
                        info,
                        &query_sources,
                        &context_sources,
                        &document_frequency,
                        df_bound,
                    )
                });
                let entry = admitted.entry(target).or_default();
                entry.push(format!("required by {}", graph.skills[*seed].name));
                if !is_seed {
                    // A requires target with no query signal has no stored
                    // Evidence; materialize a zeroed one so the necessity
                    // bump and recommendation emission behave exactly as
                    // before the deferred-materialization change.
                    let info = infos[target].get_or_insert_with(Evidence::empty);
                    info.necessity += 2.0;
                }
            }
        }
    }

    // Body-text weak evidence applies only to admitted skills: it can
    // raise an admitted skill's rank but cannot admit a skill by itself,
    // so expanding ~1 MiB bodies for non-admitted skills is wasted work.
    // Glossary expansion is appended, never rewritten, so original text
    // still matches verbatim.
    //
    // The admitted set itself is unbounded, so scanning every admitted
    // body is O(corpus) memory traffic per call. Bound the total body
    // bytes scored per recommend() under a fixed budget, visited in
    // deterministic (index) order. Body evidence can rank but never
    // admit, so skills past the budget keep a slightly lower possible
    // rank — the admitted set itself is unaffected.
    const MAX_BODY_SCAN_BYTES: usize = 8 * 1024 * 1024;
    // term_matches cost is O(query terms × field bytes): the byte cap
    // alone would let a max-length query (1024 terms) multiply 8 MiB
    // into tens of GiB of scanning. Bound total term×byte work too.
    const MAX_BODY_TERM_WORK: usize = 256 * 1024 * 1024;
    let term_count = query_terms.len() + context_terms.len();
    let mut body_scanned = 0usize;
    let mut body_work = 0usize;
    let mut body_order: Vec<usize> = admitted.keys().copied().collect();
    body_order.sort_unstable();
    for index in body_order {
        if body_scanned >= MAX_BODY_SCAN_BYTES {
            break;
        }
        let body_len = graph.skills[index].body_text.len();
        let work = term_count.saturating_mul(body_len);
        if body_scanned + body_len > MAX_BODY_SCAN_BYTES || body_work + work > MAX_BODY_TERM_WORK {
            continue;
        }
        body_scanned += body_len;
        body_work += work;
        let Some(info) = infos[index].as_mut() else {
            continue;
        };
        let body = expand_gloss(&graph.skills[index].body_text.to_lowercase());
        info.weak += field_score(&query, &query_terms, &body, 2.0, "body", &mut info.reasons)
            + field_score(
                &context,
                &context_terms,
                &body,
                1.0,
                "context+body",
                &mut info.reasons,
            );
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

fn directly_admitted(
    info: &Evidence,
    query_sources: &HashMap<String, Rc<str>>,
    context_sources: &HashMap<String, Rc<str>>,
    document_frequency: &HashMap<String, usize>,
    df_bound: usize,
) -> bool {
    let df = |term: &str| document_frequency.get(term).copied().unwrap_or(usize::MAX);
    // Corpus-relative rarity: a term counts as specific evidence while its
    // document frequency stays within df_bound (see recommend()). df==1 made
    // admission flip the moment a second same-domain skill appeared (#170).
    let rare = |term: &str| df(term) <= df_bound;
    // The skill name is the skill's identity: two or more query terms inside
    // one name is specific regardless of corpus-wide document frequency.
    // A single name hit stays conservative (low df, meaningful length).
    let multiple_name_hits = info.name_hits.len() >= 2;
    let unique_name_hit =
        info.name_hits.len() == 1 && rare(&info.name_hits[0]) && info.name_hits[0].len() >= 4;
    // A name hit plus independent description evidence admits: the name anchors
    // identity while the description confirms topical fit. This keeps high-df
    // description terms from admitting alone but lets e.g. "PR review" reach
    // `code-review` on real-size corpora.
    let name_plus_description = !info.name_hits.is_empty() && info.description_hits.len() >= 2;
    // Description-only admission (no name anchor) requires hits from at
    // least two distinct query-term sources: a single CJK substring emits a
    // bigram cluster plus glossary terms, and those must not count as
    // independent evidence (#78).
    let description_sources: HashSet<&str> = info
        .description_hits
        .iter()
        .map(|term| {
            query_sources
                .get(term)
                .map(|s| &**s)
                .unwrap_or(term.as_str())
        })
        .collect();
    let description_admitted = info.description_hits.len() >= 2
        && description_sources.len() >= 2
        && info.description_hits.iter().any(|term| rare(term));
    // Context admission applies the same independence rule: hits must
    // come from at least two distinct provenance sources, so one CJK
    // run's bigram cluster cannot self-admit through context (#144).
    let context_hit_sources: HashSet<&str> = info
        .context_hits
        .iter()
        .map(|term| {
            context_sources
                .get(term)
                .map(|s| &**s)
                .unwrap_or(term.as_str())
        })
        .collect();
    let context_admitted = info.context_hits.len() >= 2 && context_hit_sources.len() >= 2;
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
    terms: impl Iterator<Item = &'a str>,
    match_work: &mut usize,
) -> HashMap<String, usize> {
    let mut unique = HashSet::new();
    for term in terms {
        unique.insert(term);
    }
    // Deterministic degradation order under budget exhaustion.
    let mut unique: Vec<&str> = unique.into_iter().collect();
    unique.sort_unstable();
    // df counts distinct skill NAMES but the scan walks every installed
    // COPY of each candidate name — charge the bytes actually scanned
    // (all parse-ok skills) or a duplicate-heavy corpus makes the
    // budget meaningless: per-name charging under-counts by the copy
    // factor and the term loop would never trip (#144 round-12).
    let field_bytes: usize = graph
        .skills
        .iter()
        .enumerate()
        .filter(|(_, s)| s.parse_error.is_none())
        .map(|(i, _)| skill_names[i].len() + skill_descriptions[i].len())
        .sum();
    let mut frequencies = HashMap::new();
    for term in unique {
        *match_work = match_work.saturating_add(field_bytes);
        if *match_work > MAX_MATCH_TERM_WORK {
            break;
        }
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
                        && (term_matches(&skill_names[copy], term)
                            || term_matches(&skill_descriptions[copy], term))
                })
            })
            .count();
        frequencies.insert(term.to_string(), count);
    }
    frequencies
}

/// Per-skill reason strings are retained in `Evidence` for every
/// admitted candidate; a query with thousands of terms would otherwise
/// produce thousands of reason entries per skill (#144 r15).
const MAX_REASONS_PER_SKILL: usize = 64;

/// `field` must already be lowercase — callers pass pre-lowered
/// descriptions/bodies/names. Lowercasing inside this per-skill hot loop
/// would copy up to ~1 MiB per field per recommend() call.
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
    let mut score = 0.0;
    for term in query_terms {
        if term_matches(field, term) {
            score += weight;
            if reasons.len() < MAX_REASONS_PER_SKILL {
                reasons.push(format!("{label}: {term}"));
            }
        }
    }
    if !query.contains(char::is_whitespace) && term_matches(field, query) {
        score += weight * 2.0;
        if reasons.len() < MAX_REASONS_PER_SKILL {
            reasons.push(format!("{label}: {query}"));
        }
    }
    score
}

fn matches_query(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    // Link targets are scanned once per candidate skill per call; skip
    // the lowercase allocation when the target is already lowercase
    // ASCII (the common case: valid skill names are lowercase).
    if target.is_ascii() && !target.bytes().any(|b| b.is_ascii_uppercase()) {
        term_matches(target, query)
    } else {
        term_matches(&target.to_lowercase(), query)
    }
}

/// Single matching rule for query terms against skill text (#170, audit
/// I-02):
/// - ASCII terms match on word boundaries — the match must not be flanked by
///   ASCII alphanumerics, with a single trailing plural `s`/`es` tolerated so
///   "changelog" still reaches "changelogs". This stops "pr" from hitting
///   "improve"/"project"/"production" and "test" from hitting "latest".
///   Hyphens, underscores, dots, and whitespace all count as boundaries, so
///   "code-naming" still answers "code naming".
/// - Non-ASCII terms (CJK bigrams) keep substring semantics: CJK has no word
///   boundaries and bigrams are the matching unit by design (#61).
///
/// Neighbors are checked with `is_ascii_alphanumeric`, so a CJK character
/// next to ASCII (e.g. "PRをレビュー") counts as a boundary and abbreviations
/// inside Japanese text remain reachable.
fn term_matches(field: &str, term: &str) -> bool {
    if term.is_ascii() {
        ascii_word_match(field, term)
    } else {
        field.contains(term)
    }
}

fn ascii_word_match(field: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    field.match_indices(needle).any(|(start, found)| {
        let end = start + found.len();
        let left_ok = field[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if !left_ok {
            return false;
        }
        let rest = &field[end..];
        let mut chars = rest.chars();
        match chars.next() {
            None => true,
            Some(c) if !c.is_ascii_alphanumeric() => true,
            // Plural tolerance: "changelog" matches "changelogs" and
            // "class" matches "classes".
            Some('s') => chars.next().is_none_or(|c| !c.is_ascii_alphanumeric()),
            Some('e') if rest.starts_with("es") => rest[2..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric()),
            _ => false,
        }
    })
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
    "want",
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

#[cfg(test)]
fn terms(text: &str) -> Vec<String> {
    terms_tagged(text)
        .into_iter()
        .map(|(term, _)| term)
        .collect()
}

// term -> provenance source. Glossary-derived terms share the CJK key they
// came from; all other terms are their own source. Sources are Rc-shared:
// a 32K-char CJK run emits ~32K bigrams that all point at the SAME run/gap
// source, so cloning the source string per term would be O(run²) memory
// before MAX_RECOMMEND_TERMS ever applies (#144 r13).
fn term_sources(tagged: &[(String, Rc<str>)]) -> HashMap<String, Rc<str>> {
    let mut sources = HashMap::new();
    for (term, source) in tagged {
        sources
            .entry(term.clone())
            .or_insert_with(|| source.clone());
    }
    sources
}

fn terms_tagged(text: &str) -> Vec<(String, Rc<str>)> {
    let mut out: Vec<(String, Rc<str>)> = Vec::new();
    let mut ascii_run = String::new();
    let mut cjk_run = String::new();
    let mut flush = |ascii: &mut String, cjk: &mut String| {
        if ascii.chars().count() >= 2 {
            let lower = ascii.to_lowercase();
            if !STOPWORDS.contains(&lower.as_str()) {
                out.push((lower.clone(), Rc::from(lower.as_str())));
            }
        }
        ascii.clear();
        // CJK has no whitespace and no case: emit character bigrams so
        // substring matching works against Japanese descriptions (#61),
        // plus English glossary terms for known dev vocabulary so queries
        // can reach English-only skill text (#78). Provenance is scoped
        // to the evidence unit: a bigram inside a recognized dictionary
        // word shares that word's source, bigrams inside a contentful
        // uncovered gap (kanji/katakana present) share that gap as ONE
        // source, pure-hiragana gaps (particles, する) and boundary-
        // spanning bigrams are dropped, and gloss expansions keep their
        // key. One CJK word's literal bigrams and its gloss expansion
        // are therefore ONE evidence unit and cannot satisfy the
        // two-independent-sources admission rule by themselves;
        // distinct dictionary words or contentful gaps in one run
        // remain independent (#144).
        let run: String = cjk.chars().collect();
        // Interned once per run: every bigram in a keyless run shares
        // this single allocation instead of cloning ~96KB per term.
        let run_src: Rc<str> = Rc::from(run.as_str());
        let gloss = gloss_terms(&run);
        // Coverage keys come from GLOSS membership, NOT from gloss_terms
        // output: term-level dedup can hide a key whose expansions were
        // all already emitted by another key, leaving its chars
        // "uncovered" and minting spurious gap provenance (#144 r12).
        let keys: Vec<&'static str> = gloss_keys(&run);
        let chars: Vec<char> = run.chars().collect();
        // cover_key[i] = index into `keys` owning char i, or -1. Longer
        // keys claim shared chars first so プルリクエスト wins over
        // プルリク on the overlap. Work is bounded by total key-match
        // length, not run length.
        let mut cover_key = vec![-1i32; chars.len()];
        // effective_key[ki] = index of the key whose source gloss
        // expansions of keys[ki] should carry. Computed below.
        let mut effective_key: Vec<usize> = Vec::new();
        if !keys.is_empty() {
            // Byte offset -> char index; match_indices only yields
            // char-boundary starts, so interior bytes are never read.
            let mut byte_to_char = vec![0usize; run.len() + 1];
            for (ci, (b, _)) in run.char_indices().enumerate() {
                byte_to_char[b] = ci;
            }
            let mut order: Vec<usize> = (0..keys.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(keys[i].chars().count()));
            for &ki in &order {
                let key = keys[ki];
                for (b, _) in run.match_indices(key) {
                    let start = byte_to_char[b];
                    let end = (start + key.chars().count()).min(chars.len());
                    for slot in cover_key.iter_mut().take(end).skip(start) {
                        if *slot < 0 {
                            *slot = ki as i32;
                        }
                    }
                }
            }
            // A key whose EVERY occurrence is subsumed by longer keys
            // has no independent literal evidence: its gloss expansions
            // must inherit the covering key's source, or the literal
            // bigrams and the expansions of ONE dictionary word mint two
            // "independent" sources (`根本原因` containing `原因` would
            // self-admit a skill describing just `原因`). A key with at
            // least one self-covered occurrence keeps its own source.
            effective_key = vec![0usize; keys.len()];
            for (ki, key) in keys.iter().enumerate() {
                let klen = key.chars().count();
                let mut self_covered = false;
                let mut covering = ki;
                for (b, _) in run.match_indices(key) {
                    let start = byte_to_char[b];
                    let end = (start + klen).min(chars.len());
                    if cover_key[start..end].iter().all(|&c| c == ki as i32) {
                        self_covered = true;
                        break;
                    }
                    if covering == ki && cover_key[start] >= 0 {
                        covering = cover_key[start] as usize;
                    }
                }
                effective_key[ki] = if self_covered { ki } else { covering };
            }
        }
        // Gap segments: maximal runs of uncovered chars. A gap is
        // "contentful" when it contains a non-hiragana CJK char (kanji
        // or katakana) — pure-hiragana stretches are particles and
        // okurigana, i.e. grammatical filler that must not become an
        // independent evidence unit (`レビューする` stays ONE source).
        // Each contentful gap is itself a single evidence unit, so
        // `不具合報告をトリアージして` yields key + gap = two sources.
        let mut gap_of = vec![usize::MAX; chars.len()];
        let mut gap_text: Vec<Rc<str>> = Vec::new();
        let mut gap_contentful: Vec<bool> = Vec::new();
        {
            let mut i = 0;
            while i < chars.len() {
                if cover_key[i] >= 0 {
                    i += 1;
                    continue;
                }
                let start = i;
                let mut contentful = false;
                let mut text = String::new();
                while i < chars.len() && cover_key[i] < 0 {
                    text.push(chars[i]);
                    if !is_hiragana(chars[i]) {
                        contentful = true;
                    }
                    i += 1;
                }
                let id = gap_text.len();
                gap_text.push(Rc::from(text.as_str()));
                gap_contentful.push(contentful);
                for slot in gap_of.iter_mut().take(i).skip(start) {
                    *slot = id;
                }
            }
        }
        for (w, window) in chars.windows(2).enumerate() {
            let bigram: String = window.iter().collect();
            // A bigram gets key provenance only when BOTH chars belong
            // to the same key occurrence — positional coverage also fixes
            // the substring check's cross-position misattribution (a
            // bigram merely *equal* to key text elsewhere in the run no
            // longer inherits that key's source).
            if cover_key[w] >= 0
                && cover_key[w] == cover_key[w + 1]
                && keys[cover_key[w] as usize].contains(bigram.as_str())
            {
                out.push((bigram, Rc::from(keys[cover_key[w] as usize])));
            } else if keys.is_empty() {
                // Only a run with NO recognized dictionary word falls
                // back to run-level provenance.
                out.push((bigram, run_src.clone()));
            } else if cover_key[w] < 0
                && cover_key[w + 1] < 0
                && gap_of[w] == gap_of[w + 1]
                && gap_contentful[gap_of[w]]
            {
                // Both chars sit inside the same contentful gap: the
                // whole gap is one evidence unit.
                out.push((bigram, gap_text[gap_of[w]].clone()));
            }
            // else: bigram inside a keyed run that is covered, spans a
            // key/gap boundary, or sits in a pure-hiragana gap —
            // dropped so it cannot mint an independent source.
        }
        // Gloss expansions carry the EFFECTIVE source of their key: a
        // key fully subsumed by a longer dictionary word inherits that
        // word's provenance so literal+expansion of one word cannot
        // count as two independent sources.
        let key_index: HashMap<&str, usize> =
            keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        for (gloss, key) in gloss {
            if !STOPWORDS.contains(&gloss.as_str()) {
                let source = key_index
                    .get(key.as_str())
                    .map(|&ki| keys[effective_key[ki]])
                    .unwrap_or(key.as_str());
                out.push((gloss, Rc::from(source)));
            }
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
    // Common abbreviation expansion: "pull request" is routinely shortened
    // to "PR"/"pr" in skill text, so emit the abbreviation as an extra term.
    // Boundary-aware matching keeps "pr" from hitting "improve"/"project",
    // but it still matches a standalone "pr" token such as "PR review".
    let lower = text.to_lowercase();
    if lower.contains("pull request")
        || lower.contains("pull-request")
        || lower.contains("pull_request")
    {
        out.push(("pr".to_string(), Rc::from("pull-request")));
    }
    // Repeated terms would inflate hit counts and scores; keep first
    // occurrence of each term. Provenance prefers an explicitly typed
    // (self-sourced) occurrence over a glossary-derived one: when a CJK
    // run expands to "bug" and the user also typed "bug", the literal
    // term is independent evidence and must keep its own source (#144).
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut deduped: Vec<(String, Rc<str>)> = Vec::with_capacity(out.len());
    for (term, source) in out {
        if let Some(&i) = index_of.get(&term) {
            if source.as_ref() == term {
                deduped[i].1 = Rc::from(term.as_str());
            }
            continue;
        }
        index_of.insert(term.clone(), deduped.len());
        deduped.push((term, source));
    }
    deduped
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0x1100..=0x11FF
    )
}

/// Hiragana (U+3040–U+309F): particles, okurigana, and inflectional
/// endings. A CJK gap made only of these is grammatical filler, not a
/// content word, so it must not count as independent evidence.
fn is_hiragana(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x309F)
}

// Minimal Japanese↔English dev glossary for #78. This is deliberately
// small and hand-maintained: it covers high-frequency katakana loanwords
// and common kanji compounds in this domain, not general Japanese. Each
// entry maps a CJK substring to English terms appended to match text
// (both directions via the same table). All matching keys contribute.
const GLOSS: &[(&str, &str)] = &[
    ("リリースノート", "release notes changelog"),
    ("プルリクエスト", "pull request pr"),
    ("プルリク", "pull request pr"),
    ("コードレビュー", "code review"),
    ("コミットメッセージ", "commit message"),
    ("ベストプラクティス", "best practice"),
    ("テクニカルデット", "tech debt technical debt"),
    ("オブザーバビリティ", "observability"),
    ("オーケストレーション", "orchestration"),
    ("スケーラビリティ", "scalability"),
    ("パフォーマンス", "performance"),
    ("デッドロック", "deadlock"),
    ("フォールバック", "fallback"),
    ("ポストモーテム", "postmortem"),
    ("バックアップ", "backup"),
    ("リファクタリング", "refactoring refactor"),
    ("リファクタ", "refactor refactoring"),
    ("マイグレーション", "migration"),
    ("インターフェース", "interface"),
    ("コンポーネント", "component"),
    ("フレームワーク", "framework"),
    ("アーキテクチャ", "architecture"),
    ("データベース", "database db"),
    ("ドキュメンテーション", "documentation"),
    ("ドキュメント", "documentation docs"),
    ("ライブラリ", "library"),
    ("サンドボックス", "sandbox"),
    ("チェックアウト", "checkout"),
    ("ロールバック", "rollback"),
    ("リバート", "revert"),
    ("バージョン", "version"),
    ("不具合", "bug defect"),
    ("脆弱性", "vulnerability"),
    ("インシデント", "incident"),
    ("障害", "incident outage failure"),
    ("セキュリティ", "security"),
    ("環境変数", "env environment variable"),
    ("依存関係", "dependencies dependency"),
    ("リリース候補", "release candidate rc"),
    ("タイムアウト", "timeout"),
    ("レビュー", "review"),
    ("テスト", "test testing"),
    ("デバッグ", "debug"),
    ("デプロイ", "deploy deployment"),
    ("ビルド", "build"),
    ("コミット", "commit"),
    ("プッシュ", "push"),
    ("マージ", "merge"),
    ("ブランチ", "branch"),
    ("フェッチ", "fetch"),
    ("リベース", "rebase"),
    ("ワークツリー", "worktree"),
    ("バグ", "bug"),
    ("暗号化", "encryption encrypt"),
    ("認証", "auth authentication"),
    ("認可", "authorization"),
    ("権限", "permission"),
    ("ユーザー", "user"),
    ("アカウント", "account"),
    ("セッション", "session"),
    ("パスワード", "password"),
    ("設定", "config configuration settings"),
    ("構成", "config configuration"),
    ("変数", "variable"),
    ("関数", "function"),
    ("メソッド", "method"),
    ("クラス", "class"),
    ("コマンド", "command"),
    ("スクリプト", "script"),
    ("ツール", "tool"),
    ("ユーティリティ", "utility"),
    ("サービス", "service"),
    ("システム", "system"),
    ("プラットフォーム", "platform"),
    ("環境", "environment"),
    ("サーバー", "server"),
    ("クライアント", "client"),
    ("ネットワーク", "network"),
    ("データ", "data"),
    ("ファイル", "file"),
    ("ディレクトリ", "directory"),
    ("パス", "path"),
    ("キャッシュ", "cache"),
    ("キュー", "queue"),
    ("ジョブ", "job"),
    ("タスク", "task"),
    ("ワーカー", "worker"),
    ("プロセス", "process"),
    ("スレッド", "thread"),
    ("非同期", "async asynchronous"),
    ("並列", "parallel"),
    ("同期", "sync synchronous"),
    ("リトライ", "retry"),
    ("依存", "dependency"),
    ("仕様", "spec specification"),
    ("要件", "requirement"),
    ("機能", "feature"),
    ("品質", "quality"),
    ("設計", "design"),
    ("実装", "implementation"),
    ("検証", "verification validate"),
    ("確認", "check verify"),
    ("評価", "evaluation"),
    ("分析", "analysis analyze"),
    ("調査", "investigation research"),
    ("診断", "diagnosis diagnose"),
    ("命名", "naming name"),
    ("リネーム", "rename"),
    ("作成", "create"),
    ("編集", "edit"),
    ("更新", "update"),
    ("削除", "delete remove"),
    ("追加", "add"),
    ("移動", "move"),
    ("変換", "convert transform"),
    ("生成", "generate"),
    ("読み込み", "read load"),
    ("書き込み", "write"),
    ("書い", "write writing"),
    ("書く", "write writing"),
    ("表示", "display show"),
    ("検索", "search"),
    ("クエリ", "query"),
    ("結果", "result"),
    ("成功", "success"),
    ("失敗", "failure fail"),
    ("完了", "complete done"),
    ("開始", "start launch"),
    ("停止", "stop"),
    ("再起動", "restart"),
    ("インストール", "install"),
    ("セットアップ", "setup"),
    ("アンインストール", "uninstall"),
    ("初期化", "init initialize"),
    ("プロジェクト", "project"),
    ("リポジトリ", "repository repo"),
    ("コード", "code"),
    ("コメント", "comment"),
    ("ガイド", "guide"),
    ("チュートリアル", "tutorial"),
    ("ハンドブック", "handbook"),
    ("ポリシー", "policy"),
    ("ルール", "rule"),
    ("規約", "convention standards"),
    ("標準", "standard"),
    ("原則", "principle"),
    ("パターン", "pattern"),
    ("構造", "structure"),
    ("責務", "responsibility"),
    ("階層", "hierarchy"),
    ("境界", "boundary"),
    ("抽象", "abstract abstraction"),
    ("詳細", "detail"),
    ("概要", "overview"),
    ("日本語", "japanese"),
    ("自然", "natural"),
    ("英語", "english"),
    ("翻訳", "translation translate"),
    ("文章", "prose writing"),
    ("文書", "document"),
    ("文体", "style writing"),
    ("表現", "expression wording"),
    ("推敲", "polish revise"),
    ("校正", "proofread"),
    ("監査", "audit"),
    ("審査", "review audit"),
    ("承認", "approval approve"),
    ("提案", "proposal suggest"),
    ("改善", "improvement improve"),
    ("最適化", "optimization optimize"),
    ("効率", "efficiency"),
    ("コスト", "cost"),
    ("リソース", "resource"),
    ("容量", "capacity"),
    ("制限", "limit"),
    ("閾値", "threshold"),
    ("条件", "condition"),
    ("制約", "constraint"),
    ("不変条件", "invariant"),
    ("契約", "contract"),
    ("互換性", "compatibility"),
    ("移行", "migration migrate"),
    ("廃止", "deprecate"),
    ("非推奨", "deprecated"),
    ("安定", "stable"),
    ("実験", "experiment"),
    ("ベータ", "beta"),
    ("本番", "production"),
    ("開発", "development dev"),
    ("運用", "operations"),
    ("保守", "maintenance"),
    ("監視", "monitoring monitor"),
    ("通知", "notification"),
    ("警告", "warning"),
    ("アラート", "alert"),
    ("メトリクス", "metrics"),
    ("ダッシュボード", "dashboard"),
    ("レポート", "report"),
    ("履歴", "history"),
    ("記録", "record log"),
    ("観測", "observation observe"),
    ("計測", "measurement"),
    ("ベンチマーク", "benchmark"),
    ("性能", "performance"),
    ("速度", "speed performance"),
    ("メモリ", "memory"),
    ("ストレージ", "storage"),
    ("ディスク", "disk"),
    ("帯域", "bandwidth"),
    ("遅延", "latency delay"),
    ("スループット", "throughput"),
    ("負荷", "load"),
    ("可用性", "availability"),
    ("信頼性", "reliability"),
    ("耐障害性", "fault tolerance"),
    ("冗長", "redundancy"),
    ("クラスタ", "cluster"),
    ("分散", "distributed distribution"),
    ("スケール", "scale"),
    ("インフラ", "infrastructure infra"),
    ("コンテナ", "container"),
    ("仮想", "virtual"),
    ("マシン", "machine"),
    ("デバイス", "device"),
    ("端末", "device terminal"),
    ("競合", "conflict race"),
    ("ロック", "lock"),
    ("排他", "exclusive mutex"),
    ("共有", "shared"),
    ("プライベート", "private"),
    ("公開", "public"),
    ("内部", "internal"),
    ("外部", "external"),
    ("秘密", "secret"),
    ("機密", "confidential sensitive"),
    ("漏洩", "leak breach"),
    ("侵害", "breach compromise"),
    ("脅威", "threat"),
    ("攻撃", "attack"),
    ("防御", "defense"),
    ("マルウェア", "malware"),
    ("ウイルス", "virus"),
    ("フィッシング", "phishing"),
    ("暗号", "crypto cryptography"),
    ("ハッシュ", "hash"),
    ("署名", "signature sign"),
    ("証明書", "certificate cert"),
    ("鍵", "key"),
    ("復号", "decryption decrypt"),
    ("復旧", "recovery recover"),
    ("原因", "cause root cause"),
    ("根本原因", "root cause"),
    ("再現", "reproduce reproduction"),
    ("修正", "fix"),
    ("パッチ", "patch"),
    ("ホットフィックス", "hotfix"),
    ("回帰", "regression"),
    ("デグレ", "regression degrade"),
    ("互換", "compatible compatibility"),
    ("後方互換", "backward compatibility"),
    ("リリース", "release"),
    ("ノート", "note"),
    ("変更", "change"),
    ("差分", "diff"),
    ("コンフリクト", "conflict"),
    ("プル", "pull"),
    ("クローン", "clone"),
    ("サブモジュール", "submodule"),
    ("ステージング", "staging"),
    ("インデックス", "index"),
    ("メイン", "main"),
    ("マスター", "master main"),
    ("デフォルト", "default"),
    ("ベース", "base"),
    ("ターゲット", "target"),
    ("ソース", "source"),
    ("オリジン", "origin upstream"),
    ("アップストリーム", "upstream"),
    ("フォーク", "fork"),
    ("解決", "resolve"),
    ("クローズ", "close closed"),
    ("リオープン", "reopen"),
    ("ラベル", "label"),
    ("マイルストーン", "milestone"),
    ("アサイン", "assign"),
    ("担当", "assignee owner"),
    ("レビュアー", "reviewer"),
    ("差し戻し", "request changes"),
    ("マージ可能", "mergeable"),
    ("自動マージ", "auto merge"),
    ("スカッシュ", "squash"),
    ("チェリーピック", "cherry-pick"),
    ("リセット", "reset"),
    ("ツリー", "tree"),
    ("オブジェクト", "object"),
    ("ブロブ", "blob"),
    ("ダイジェスト", "digest"),
    ("整合性", "integrity"),
    ("チェックサム", "checksum"),
    ("トークン", "token"),
    ("ログ", "log logging"),
];

/// Every glossary key present in `text`, independent of term-level
/// dedup: `gloss_terms` dedups by emitted term, so a key whose gloss
/// was fully claimed by an earlier key would otherwise be invisible to
/// provenance coverage even though its chars are a recognized word.
fn gloss_keys(text: &str) -> Vec<&'static str> {
    if text.is_ascii() {
        return Vec::new();
    }
    GLOSS
        .iter()
        .filter(|(key, gloss)| !gloss.is_empty() && text.contains(key))
        .map(|(key, _)| *key)
        .collect()
}

// Returns (English term, CJK key) pairs for every glossary key present in
// `text`, deduplicated by term. Used in both directions: query terms emit
// these into the term list, and skill match text gets the terms appended.
fn gloss_terms(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    // Every glossary key is CJK; ASCII-only text can never contain one,
    // so skip the ~290-entry scan for English descriptions/bodies.
    if text.is_ascii() {
        return out;
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (key, gloss) in GLOSS {
        if gloss.is_empty() || !text.contains(key) {
            continue;
        }
        for term in gloss.split_whitespace() {
            if seen.insert(term.to_string()) {
                out.push((term.to_string(), key.to_string()));
            }
        }
    }
    out
}

// Appends English glossary expansions for every CJK substring in `text`.
// Original text is preserved verbatim; expansions are appended so CJK
// descriptions remain reachable from English query terms and vice versa.
fn expand_gloss(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    out.push_str(text);
    for (term, _) in gloss_terms(text) {
        out.push(' ');
        out.push_str(&term);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{expand_gloss, term_sources, terms, terms_tagged};

    #[test]
    fn cjk_query_produces_bigrams() {
        // A run with no glossary key still emits every bigram with
        // run-level provenance.
        let t = terms("あいうえおかきくけこ");
        assert!(t.iter().any(|x| x == "あい"), "{t:?}");
        assert!(t.iter().any(|x| x == "けこ"), "{t:?}");
        // Inside a keyed run, bigrams inside a contentful uncovered gap
        // survive (shared gap provenance) while pure-hiragana filler
        // like する is dropped so it cannot mint an independent source.
        let t = terms("不具合報告をトリアージして");
        assert!(t.iter().any(|x| x == "報告"), "{t:?}");
        let t = terms("レビューする");
        assert!(!t.iter().any(|x| x == "する"), "{t:?}");
    }

    #[test]
    fn cjk_query_emits_glossary_terms() {
        let t = terms("PRをレビューして");
        assert!(t.iter().any(|x| x == "review"), "{t:?}");
        let t = terms("新しい関数の命名を考えて");
        assert!(t.iter().any(|x| x == "naming"), "{t:?}");
        assert!(t.iter().any(|x| x == "function"), "{t:?}");
    }

    #[test]
    fn cjk_provenance_splits_bigrams_from_gloss_keys() {
        let tagged = terms_tagged("文章を自然にしたい");
        let sources = term_sources(&tagged);
        // Round-10/11: provenance is scoped to the evidence unit — a
        // bigram inside a recognized dictionary word shares that word's
        // source (so the word's literal bigrams and its gloss expansion
        // count as ONE evidence unit), and distinct words in one run
        // stay independent. Boundary-spanning bigrams and pure-hiragana
        // gaps are dropped entirely — giving them run-level provenance
        // would recreate the `レビューする` self-admission hole.
        assert_eq!(sources.get("文章").map(|s| &**s), Some("文章"));
        assert_eq!(sources.get("章を"), None);
        assert_eq!(sources.get("自然").map(|s| &**s), Some("自然"));
        // A contentful gap is a single evidence unit: every bigram in
        // `報告をトリアージして` shares the gap text as its source.
        let tagged = terms_tagged("不具合報告をトリアージして");
        let gap_sources = term_sources(&tagged);
        let gap = "報告をトリアージして";
        assert_eq!(gap_sources.get("報告").map(|s| &**s), Some(gap));
        assert_eq!(gap_sources.get("トリ").map(|s| &**s), Some(gap));
        assert_eq!(gap_sources.get("不具").map(|s| &**s), Some("不具合"));
        assert_eq!(sources.get("prose").map(|s| &**s), Some("文章"));
        assert_eq!(sources.get("natural").map(|s| &**s), Some("自然"));
    }

    /// #144 round-12 (CodeRabbit): a dictionary key fully contained in a
    /// longer key must not keep an independent source for its gloss
    /// expansions — `原因` inside `根本原因` is ONE evidence unit, so a
    /// skill whose description is just `原因` (literal + `root cause`
    /// expansion) cannot self-admit on query `根本原因`.
    #[test]
    fn contained_key_gloss_inherits_covering_source() {
        let tagged = terms_tagged("根本原因");
        let sources = term_sources(&tagged);
        // The bigram 原因 is covered by 根本原因; the gloss terms that
        // 原因 would emit (root, cause) must carry the SAME source.
        assert_eq!(sources.get("原因").map(|s| &**s), Some("根本原因"));
        assert_eq!(sources.get("root").map(|s| &**s), Some("根本原因"));
        assert_eq!(sources.get("cause").map(|s| &**s), Some("根本原因"));
        // A standalone 原因 elsewhere in the run is a genuinely
        // independent dictionary word and keeps its own source.
        let tagged = terms_tagged("根本原因と原因");
        let sources = term_sources(&tagged);
        assert_eq!(sources.get("原因").map(|s| &**s), Some("原因"));
        assert_eq!(sources.get("root").map(|s| &**s), Some("原因"));
        assert_eq!(sources.get("根本").map(|s| &**s), Some("根本原因"));
    }

    #[test]
    fn expand_gloss_appends_without_rewriting() {
        let out = expand_gloss("命名と設計を扱う");
        assert!(out.starts_with("命名と設計を扱う"));
        assert!(out.contains("naming"));
        assert!(out.contains("design"));
        // ASCII-only text is unchanged.
        assert_eq!(expand_gloss("code review"), "code review");
    }

    #[test]
    fn pull_request_phrase_emits_pr() {
        let t = terms("review this pull request");
        assert!(t.iter().any(|x| x == "pr"), "{t:?}");
        let t = terms("review the changes");
        assert!(!t.iter().any(|x| x == "pr"), "{t:?}");
    }

    #[test]
    fn modal_filler_is_stopworded() {
        // #184: "want" is modal filler like "please"; letting it match
        // description text admitted unrelated skills (e.g. code-review via
        // "the user wants") on verification-setup phrasing.
        let t = terms("i want a repeatable way to confirm the project works");
        assert!(!t.iter().any(|x| x == "want"), "{t:?}");
    }
}
