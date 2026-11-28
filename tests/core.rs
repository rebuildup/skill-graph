use skill_graph::discovery::discover;
use skill_graph::model::Availability;
use skill_graph::parser::parse_skill;
use skill_graph::resolver::build;
use skill_graph::search::recommend;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/skills")
}

#[test]
fn discovers_fixture_skills() {
    let discovery = discover(&[fixture_root()]);
    assert_eq!(discovery.skills.len(), 9);
    assert!(discovery.errors.is_empty());
}

#[test]
fn resolves_links_requires_and_backlinks() {
    let graph = build(discover(&[fixture_root()]));
    let alpha = graph.by_name["alpha-skill"][0];
    let beta = graph.by_name["beta-skill"][0];
    let gamma = graph.by_name["gamma-skill"][0];

    let alpha_links = &graph.skills[alpha].links;
    assert!(
        alpha_links
            .iter()
            .any(|l| l.kind == skill_graph::model::LinkKind::Markdown
                && l.resolved_name.as_deref() == Some("beta-skill"))
    );
    assert!(
        alpha_links
            .iter()
            .any(|l| l.kind == skill_graph::model::LinkKind::Requires
                && l.resolved_name.as_deref() == Some("beta-skill"))
    );
    // backlinks stores deduplicated source counts, not source lists
    // (#144 r18): alpha reaches beta via Markdown + Requires and gamma
    // via a body mention, so each target counts alpha exactly once.
    assert_eq!(graph.backlinks.get(&beta), Some(&1));
    assert_eq!(graph.backlinks.get(&gamma), Some(&1));
}

#[test]
fn recommends_with_reasons_and_empty_when_no_match() {
    let graph = build(discover(&[fixture_root()]));
    let recs = recommend(&graph, "release notes changelog", "version announcement", 3);
    assert_eq!(
        recs.iter()
            .map(|r| r.skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["gamma-skill"]
    );
    assert!(!recs[0].reasons.is_empty());
    assert!(recommend(&graph, "quantum gardening", "", 3).is_empty());
}

#[test]
fn keywords_bridge_paraphrased_task_phrasing() {
    // #184: natural phrasing that avoids description vocabulary still
    // reaches the skill through opt-in `keywords` aliases.
    let graph = build(discover(&[fixture_root()]));
    let recs = recommend(
        &graph,
        "we need to tell customers what changed in tonight's hotfix — draft the announcement",
        "",
        3,
    );
    assert_eq!(recs[0].skill.name, "gamma-skill");
}

#[test]
fn single_rare_description_term_admits_at_corpus_scale() {
    // #319: a real task prompt may share exactly one content word with a
    // skill's description vocabulary. Requiring two description hits made
    // recommend() abstain on "--verbose and --output FILE flags" even
    // though "flags" is a corpus-rare, meaningful term.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(
        &root.join("cli-arg-parsing"),
        "cli-arg-parsing",
        "Design command-line interfaces with clear flags, subcommands, help text, and validation. Use when adding CLI options, arguments, or a new command-line tool.",
    );
    // Distractors carry the prompt's other vocabulary so the target's
    // "flags" hit stays the only description evidence it receives.
    for i in 0..24 {
        write_skill_md(
            &root.join(format!("distractor-{i:02}")),
            &format!("distractor-{i:02}"),
            &format!("Reference for distractor topic number {i} covering reports, files, users, and lists."),
        );
    }
    let graph = build(discover(&[root]));
    let recs = recommend(
        &graph,
        "Create tools/report.py that prints the user list from app/main.py, with --verbose and --output FILE flags.",
        "",
        5,
    );
    assert!(
        recs.iter().any(|r| r.skill.name == "cli-arg-parsing"),
        "expected cli-arg-parsing in recommendations, got: {:?}",
        recs.iter().map(|r| &r.skill.name).collect::<Vec<_>>()
    );
}

#[test]
fn single_common_description_term_does_not_admit() {
    // Guard for the #319 relaxation: a lone hit only admits when the term
    // is corpus-rare. A term shared across many descriptions stays out.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    for i in 0..24 {
        write_skill_md(
            &root.join(format!("common-{i:02}")),
            &format!("common-{i:02}"),
            "Guidance for handling files and reports in production systems.",
        );
    }
    let graph = build(discover(&[root]));
    assert!(recommend(&graph, "handle the files", "", 5).is_empty());
}

#[test]
fn stopword_and_punctuation_only_queries_do_not_recommend() {
    // Regression for #46: generic English function words and punctuation
    // matched description text and admitted irrelevant skills on real
    // non-task prompts during S02 live verification.
    let graph = build(discover(&[fixture_root()]));
    for query in [
        "the and of to in for with it is on",
        "What is 17 * 23? Answer with just the number.",
        "-> -> ->",
        "the",
    ] {
        assert!(
            recommend(&graph, query, "", 5).is_empty(),
            "query should produce no recommendations: {query}"
        );
    }
}

fn write_skill_md(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n"),
    )
    .unwrap();
}

#[test]
fn digest_is_stable_and_content_derived() {
    let root = fixture_root();
    let path = root.join("alpha-skill/SKILL.md");
    let first = parse_skill(&path, &root).unwrap();
    let second = parse_skill(&path, &root).unwrap();
    assert_eq!(first.digest, second.digest);
    assert_eq!(first.digest.len(), 64);

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("alpha-skill");
    fs::create_dir_all(&dir).unwrap();
    let original = fs::read_to_string(&path).unwrap();
    fs::write(dir.join("SKILL.md"), format!("{original}\nextra\n")).unwrap();
    let changed = parse_skill(&dir.join("SKILL.md"), temp.path()).unwrap();
    assert_ne!(first.digest, changed.digest);
}

#[test]
fn duplicate_installs_recommend_once_and_keep_requires() {
    let temp = tempfile::tempdir().unwrap();
    for copy in ["root-a", "root-b"] {
        let root = temp.path().join(copy);
        let parent = root.join("parent-skill");
        fs::create_dir_all(&parent).unwrap();
        fs::write(
            parent.join("SKILL.md"),
            "---\nname: parent-skill\ndescription: Use for parent orchestration work.\nmetadata:\n  requires-skills: child-skill\n---\n\nbody\n",
        )
        .unwrap();
        write_skill_md(
            &root.join("child-skill"),
            "child-skill",
            "Use for child execution work.",
        );
    }
    let graph = build(discover(&[
        temp.path().join("root-a"),
        temp.path().join("root-b"),
    ]));
    assert_eq!(graph.by_name["parent-skill"].len(), 2);

    let recs = recommend(&graph, "parent-skill", "", 5);
    let names: Vec<&str> = recs.iter().map(|r| r.skill.name.as_str()).collect();
    assert_eq!(names.iter().filter(|n| **n == "parent-skill").count(), 1);
    assert!(names.contains(&"child-skill"));
    assert!(
        recs.iter()
            .find(|r| r.skill.name == "child-skill")
            .is_some_and(|r| r
                .reasons
                .iter()
                .any(|reason| reason.contains("required by")))
    );
}

#[test]
fn same_name_distinct_versions_are_flagged_not_merged_silently() {
    let temp = tempfile::tempdir().unwrap();
    write_skill_md(
        &temp.path().join("root-a/collide-skill"),
        "collide-skill",
        "Use for orchestration work.",
    );
    write_skill_md(
        &temp.path().join("root-b/collide-skill"),
        "collide-skill",
        "Use for sourdough baking work.",
    );
    let graph = build(discover(&[
        temp.path().join("root-a"),
        temp.path().join("root-b"),
    ]));

    // Query matches only the second copy's description; the name is still
    // recommended once, with the collision surfaced in reasons.
    let recs = recommend(&graph, "sourdough baking", "", 5);
    let collide: Vec<_> = recs
        .iter()
        .filter(|r| r.skill.name == "collide-skill")
        .collect();
    assert_eq!(collide.len(), 1);
    assert!(
        collide[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("name collision"))
    );

    // Resolution still reports both distinct copies, not a silent merge.
    let resolved =
        serde_json::to_value(skill_graph::cli::resolve_target(&graph, "collide-skill")).unwrap();
    assert_eq!(resolved["kind"], "name");
    assert_eq!(resolved["ambiguous"], true);
    assert_eq!(resolved["skills"].as_array().unwrap().len(), 2);
}

#[test]
fn identical_duplicate_copies_do_not_flag_collision() {
    let temp = tempfile::tempdir().unwrap();
    for copy in ["root-a", "root-b"] {
        write_skill_md(
            &temp.path().join(format!("{copy}/same-skill")),
            "same-skill",
            "Use for identical duplicated work.",
        );
    }
    let graph = build(discover(&[
        temp.path().join("root-a"),
        temp.path().join("root-b"),
    ]));
    let recs = recommend(&graph, "same-skill", "", 5);
    let same: Vec<_> = recs
        .iter()
        .filter(|r| r.skill.name == "same-skill")
        .collect();
    assert_eq!(same.len(), 1);
    assert!(
        !same[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("name collision"))
    );
}

#[test]
fn requires_to_ambiguous_name_admits_once_with_collision_note() {
    let temp = tempfile::tempdir().unwrap();
    for copy in ["root-a", "root-b"] {
        let root = temp.path().join(copy);
        let parent = root.join("parent-skill");
        fs::create_dir_all(&parent).unwrap();
        fs::write(
            parent.join("SKILL.md"),
            "---\nname: parent-skill\ndescription: Use for parent work.\nmetadata:\n  requires-skills: dep-skill\n---\n\nbody\n",
        )
        .unwrap();
        // Two genuinely different dep-skill versions.
        write_skill_md(
            &root.join("dep-skill"),
            "dep-skill",
            if copy == "root-a" {
                "Use for dep variant alpha."
            } else {
                "Use for dep variant beta."
            },
        );
    }
    let graph = build(discover(&[
        temp.path().join("root-a"),
        temp.path().join("root-b"),
    ]));
    let recs = recommend(&graph, "parent-skill", "", 5);
    let deps: Vec<_> = recs
        .iter()
        .filter(|r| r.skill.name == "dep-skill")
        .collect();
    assert_eq!(deps.len(), 1);
    assert!(deps[0].reasons.iter().any(|r| r.contains("required by")));
    assert!(deps[0].reasons.iter().any(|r| r.contains("name collision")));
}

#[test]
fn backlinks_cover_all_name_copies_and_dedup_sources() {
    // Regression for #171 (audit D-04): with two same-name copies, a
    // path-resolved Markdown link attaches to its concrete copy while a
    // name-resolved Requires link attaches to every copy; both links
    // share one source skill, so each target counts it exactly once.
    let temp = tempfile::tempdir().unwrap();
    for copy in ["root-a", "root-b"] {
        write_skill_md(
            &temp.path().join(format!("{copy}/dep-skill")),
            "dep-skill",
            "Use for dep work.",
        );
    }
    let referrer = temp.path().join("root-a/referrer-skill");
    fs::create_dir_all(&referrer).unwrap();
    fs::write(
        referrer.join("SKILL.md"),
        "---\nname: referrer-skill\ndescription: Use for referring work.\nmetadata:\n  requires-skills: dep-skill\n---\n\nSee [dep](../dep-skill).\n",
    )
    .unwrap();
    let graph = build(discover(&[
        temp.path().join("root-a"),
        temp.path().join("root-b"),
    ]));
    let deps = &graph.by_name["dep-skill"];
    assert_eq!(deps.len(), 2);
    for &target in deps {
        assert_eq!(
            graph.backlinks.get(&target),
            Some(&1),
            "target {target} must carry exactly one deduped backlink"
        );
    }
}

#[test]
fn ambiguous_names_are_reported_not_uniquified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(&root.join("one"), "dup-skill", "first duplicate");
    write_skill_md(&root.join("two"), "dup-skill", "second duplicate");
    let graph = build(discover(&[root]));
    assert!(graph.ambiguous_names.contains(&"dup-skill".to_string()));
    assert_eq!(graph.by_name["dup-skill"].len(), 2);
}

#[test]
fn cjk_terms_and_keywords_admit_crosslingual_match() {
    let graph = build(discover(&[fixture_root()]));
    let recs = recommend(&graph, "不具合報告をトリアージして", "", 5);
    assert!(
        recs.iter().any(|r| r.skill.name == "kappa-skill"),
        "expected kappa-skill via keywords, got: {:?}",
        recs.iter().map(|r| &r.skill.name).collect::<Vec<_>>()
    );
}

/// #144 round-9: one CJK substring is ONE piece of evidence. A query
/// that is a single CJK word emits a bigram cluster plus glossary
/// expansions, but they all share the run as provenance — they must not
/// satisfy the two-independent-sources description admission rule by
/// themselves. Two distinct CJK words DO provide independent sources.
#[test]
fn cjk_single_word_cannot_self_admit_via_description() {
    let graph = build(discover(&[fixture_root()]));

    // Single CJK word: bigram cluster + gloss "review" all trace to the
    // same run -> description-only admission must fail.
    let recs = recommend(&graph, "レビュー", "", 5);
    assert!(
        !recs.iter().any(|r| r.skill.name == "japanese-only-skill"),
        "single CJK substring self-admitted: {recs:?}"
    );

    // Two distinct CJK words are independent sources -> admits.
    let recs = recommend(&graph, "レビュー テスト", "", 5);
    assert!(
        recs.iter().any(|r| r.skill.name == "japanese-only-skill"),
        "two CJK words should admit: {recs:?}"
    );
}

/// #144 round-9: resolver-derived mentions are capped per skill —
/// parser MAX_LINKS does not cover them, so a body naming many skills
/// must not create unbounded link/backlink blowup.
#[test]
fn resolver_mentions_are_capped_per_skill() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..70 {
        let d = dir.path().join(format!("skill-{i:02}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: skill-{i:02}\ndescription: fixture {i}\n---\nbody\n"),
        )
        .unwrap();
    }
    let mentioner = dir.path().join("mentioner");
    std::fs::create_dir_all(&mentioner).unwrap();
    let names = (0..70)
        .map(|i| format!("skill-{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        mentioner.join("SKILL.md"),
        format!("---\nname: mentioner\ndescription: mentions many\n---\n{names}\n"),
    )
    .unwrap();

    let graph = build(discover(&[dir.path().to_path_buf()]));
    let skill = graph
        .skills
        .iter()
        .find(|s| s.name == "mentioner")
        .expect("mentioner discovered");
    let mentions = skill
        .links
        .iter()
        .filter(|l| l.kind == skill_graph::model::LinkKind::Mention)
        .count();
    assert_eq!(mentions, 64, "mention cap should engage deterministically");
}

/// #225: the per-skill mention cap applies to resolved mentions, not to
/// scanned tokens — unrelated vocabulary sorting before a real skill
/// name must not crowd the genuine reference out of the cap.
#[test]
fn resolver_mentions_ignore_unrelated_token_flood() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["real-target", "other-skill"] {
        let d = dir.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: fixture\n---\nbody\n"),
        )
        .unwrap();
    }
    let mentioner = dir.path().join("mentioner");
    std::fs::create_dir_all(&mentioner).unwrap();
    // 70 non-skill tokens that sort before "real-target"/"other-skill":
    // under the old take(64)-then-filter order they consumed the entire
    // cap and both genuine mentions were lost.
    let flood = (0..70)
        .map(|i| format!("aaa-filler-{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        mentioner.join("SKILL.md"),
        format!("---\nname: mentioner\ndescription: flood\n---\n{flood} real-target other-skill\n"),
    )
    .unwrap();

    let graph = build(discover(&[dir.path().to_path_buf()]));
    let skill = graph
        .skills
        .iter()
        .find(|s| s.name == "mentioner")
        .expect("mentioner discovered");
    let mut names: Vec<&str> = skill
        .links
        .iter()
        .filter(|l| l.kind == skill_graph::model::LinkKind::Mention)
        .map(|l| l.target.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["other-skill", "real-target"]);
}

/// #144 round-11: a CJK run that contains a recognized dictionary word
/// plus uncovered characters (particles/suffixes like する) must still
/// count as ONE evidence unit. Previously the uncovered bigrams fell
/// back to run-level provenance, giving the same utterance a second
/// source (`{key, run}`) and letting `レビューする` self-admit — the
/// hole the key-scoped bigram fix was meant to close.
#[test]
fn cjk_key_plus_particle_cannot_self_admit() {
    let graph = build(discover(&[fixture_root()]));

    // One run = glossary key レビュー + uncovered する: only the key's
    // provenance survives, so description admission (needs 2 sources)
    // must fail.
    let recs = recommend(&graph, "レビューする", "", 5);
    assert!(
        !recs.iter().any(|r| r.skill.name == "japanese-only-skill"),
        "key+particle run self-admitted: {recs:?}"
    );

    // Two independent runs still admit.
    let recs = recommend(&graph, "レビューする テスト", "", 5);
    assert!(
        recs.iter().any(|r| r.skill.name == "japanese-only-skill"),
        "independent second source should admit: {recs:?}"
    );
}

/// Round-13: a parse-error skill must never admit — including via the
/// exact-name path — so it cannot seed requires-propagation for a
/// dependency it declares. #225 B07 split spec violations out of
/// parse_error: a skill with an invalid frontmatter name is now
/// compatibility-accepted (sanitized name + spec_violations) and its
/// requires declarations work normally — this test uses a hard parse
/// failure (broken YAML) for the exclusion guarantee instead.
#[test]
fn parse_error_skill_cannot_seed_requires() {
    let temp = tempfile::tempdir().unwrap();
    let bad = temp.path().join("bad-deploy");
    fs::create_dir_all(&bad).unwrap();
    // Broken YAML frontmatter: parse_error set, no metadata survives,
    // so no requires link can exist — the skill is fully excluded.
    fs::write(
        bad.join("SKILL.md"),
        "---\nname: [unclosed\ndescription: d\n---\n\nbody\n",
    )
    .unwrap();
    write_skill_md(
        &temp.path().join("victim-skill"),
        "victim-skill",
        "Use for victim work.",
    );
    let graph = build(discover(&[temp.path().to_path_buf()]));
    assert!(graph.skills.iter().any(|s| s.parse_error.is_some()));
    let recs = recommend(&graph, "bad-deploy", "", 5);
    assert!(
        recs.iter().all(|r| r.skill.name != "victim-skill"),
        "parse-error skill must not admit its requires target: {recs:?}"
    );
}

/// #225 B07: spec violations are compatibility-accepted — the skill
/// stays discoverable/recommendable under a sanitized name with the
/// violation recorded, distinct from a hard parse error.
#[test]
fn spec_violations_are_compat_accepted_not_parse_errors() {
    let temp = tempfile::tempdir().unwrap();
    // Dir fallback yields the valid name "bad-deploy"; the frontmatter
    // name contains a newline (spec charset violation).
    let bad = temp.path().join("bad-deploy");
    fs::create_dir_all(&bad).unwrap();
    fs::write(
        bad.join("SKILL.md"),
        "---\nname: \"bad\\ndeploy\"\ndescription: bad deploy helper\n---\n\nbody\n",
    )
    .unwrap();
    // Consecutive hyphens violate the current spec name rule.
    let hyph = temp.path().join("hyph");
    fs::create_dir_all(&hyph).unwrap();
    fs::write(
        hyph.join("SKILL.md"),
        "---\nname: foo--bar\ndescription: d\n---\n\nbody\n",
    )
    .unwrap();
    // Declared name valid but != directory name.
    let mismatched = temp.path().join("real-dir");
    fs::create_dir_all(&mismatched).unwrap();
    fs::write(
        mismatched.join("SKILL.md"),
        "---\nname: other-name\ndescription: d\n---\n\nbody\n",
    )
    .unwrap();
    // Missing description.
    let nodesc = temp.path().join("nodesc");
    fs::create_dir_all(&nodesc).unwrap();
    fs::write(nodesc.join("SKILL.md"), "---\nname: nodesc\n---\n\nbody\n").unwrap();

    let graph = build(discover(&[temp.path().to_path_buf()]));
    let bad = graph
        .skills
        .iter()
        .find(|s| s.path.starts_with(&bad))
        .unwrap();
    assert_eq!(bad.name, "bad-deploy", "sanitized to dir fallback");
    assert!(bad.parse_error.is_none());
    assert!(
        bad.spec_violations
            .iter()
            .any(|v| v.contains("skill-name spec")),
        "{:?}",
        bad.spec_violations
    );

    let hyph = graph
        .skills
        .iter()
        .find(|s| s.path.starts_with(&hyph))
        .unwrap();
    // "foo--bar" is spec-invalid: sanitized to the valid dir name.
    assert_eq!(hyph.name, "hyph");
    assert!(hyph.parse_error.is_none());
    assert!(
        hyph.spec_violations
            .iter()
            .any(|v| v.contains("skill-name spec")),
        "{:?}",
        hyph.spec_violations
    );

    let mismatched = graph
        .skills
        .iter()
        .find(|s| s.path.starts_with(&mismatched))
        .unwrap();
    assert_eq!(mismatched.name, "other-name");
    assert!(mismatched.parse_error.is_none());
    assert!(
        mismatched
            .spec_violations
            .iter()
            .any(|v| v.contains("parent directory")),
        "{:?}",
        mismatched.spec_violations
    );

    let nodesc = graph
        .skills
        .iter()
        .find(|s| s.path.starts_with(&nodesc))
        .unwrap();
    assert!(nodesc.parse_error.is_none());
    assert!(
        nodesc
            .spec_violations
            .iter()
            .any(|v| v.contains("description")),
        "{:?}",
        nodesc.spec_violations
    );

    // Compat acceptance: spec-violating skills remain recommendable.
    let recs = recommend(&graph, "bad deploy helper", "", 5);
    assert!(
        recs.iter().any(|r| r.skill.name == "bad-deploy"),
        "compat-accepted skill must be recommendable: {recs:?}"
    );
    // And the violation is visible on the wire summary.
    let summary = bad.summary();
    assert!(!summary.spec_violations.is_empty());
    assert!(matches!(summary.availability, Availability::Available));
}
