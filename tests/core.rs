use skill_graph::discovery::discover;
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
fn ambiguous_names_are_reported_not_uniquified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(&root.join("one"), "dup-skill", "first duplicate");
    write_skill_md(&root.join("two"), "dup-skill", "second duplicate");
    let graph = build(discover(&[root]));
    assert!(graph.ambiguous_names.contains(&"dup-skill".to_string()));
    assert_eq!(graph.by_name["dup-skill"].len(), 2);
}
