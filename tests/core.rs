use skill_graph::discovery::discover;
use skill_graph::resolver::build;
use skill_graph::search::recommend;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/skills")
}

#[test]
fn discovers_fixture_skills() {
    let discovery = discover(&[fixture_root()]);
    assert_eq!(discovery.skills.len(), 7);
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
    assert!(
        graph
            .backlinks
            .get(&beta)
            .is_some_and(|v| v.contains(&alpha))
    );
    assert!(
        graph
            .backlinks
            .get(&gamma)
            .is_some_and(|v| v.contains(&alpha))
    );
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
