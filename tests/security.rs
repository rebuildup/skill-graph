use skill_graph::discovery::{Discovery, discover};
use skill_graph::model::LinkKind;
use skill_graph::resolver::build;
use skill_graph::search::recommend;
use std::fs;
use std::path::Path;

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .unwrap();
}

fn skill_names(discovery: &Discovery) -> Vec<String> {
    let mut names = discovery
        .skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[cfg(unix)]
#[test]
fn discovery_skips_symlinked_skill_files_and_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    fs::create_dir_all(&root).unwrap();
    write_skill(&root, "real-skill", "real skill", "body");

    let outside = temp.path().join("outside");
    write_skill(&outside, "outside-skill", "outside skill", "body");
    std::os::unix::fs::symlink(outside.join("outside-skill"), root.join("linked-skill")).unwrap();

    let bad = root.join("bad-skill");
    fs::create_dir_all(&bad).unwrap();
    std::os::unix::fs::symlink(outside.join("outside-skill/SKILL.md"), bad.join("SKILL.md"))
        .unwrap();

    let before = fs::read_dir(&root).unwrap().count();
    let discovery = discover(std::slice::from_ref(&root));
    let after = fs::read_dir(&root).unwrap().count();
    assert_eq!(before, after, "discover must not write into the skill root");
    assert_eq!(skill_names(&discovery), vec!["real-skill".to_string()]);
}

#[test]
fn markdown_links_do_not_resolve_outside_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    fs::create_dir_all(&root).unwrap();
    write_skill(
        &root,
        "skill-a",
        "alpha skill",
        "[outside](../../outside/secret/SKILL.md)",
    );
    write_skill(
        &temp.path().join("outside"),
        "secret",
        "secret skill",
        "body",
    );

    let discovery = discover(std::slice::from_ref(&root));
    assert_eq!(discovery.skills.len(), 1);
    let graph = build(discovery);
    let link = graph.skills[0]
        .links
        .iter()
        .find(|link| link.target.contains("outside"))
        .unwrap();
    assert_eq!(link.kind, LinkKind::Markdown);
    assert!(link.resolved_name.is_none());
    assert!(
        link.resolved_path
            .as_ref()
            .is_none_or(|path| !path.starts_with(&root))
    );
}

#[test]
fn external_links_are_marked_and_not_resolved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    fs::create_dir_all(&root).unwrap();
    write_skill(
        &root,
        "skill-a",
        "alpha skill",
        "[site](https://example.com/x) [mail](mailto:a@example.com)",
    );

    let graph = build(discover(&[root]));
    let kinds = graph.skills[0]
        .links
        .iter()
        .map(|link| link.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&LinkKind::External));
    assert!(
        graph.skills[0]
            .links
            .iter()
            .all(|link| link.resolved_name.is_none())
    );
}

#[test]
fn parse_errors_are_surfaced_and_skipped_by_recommend() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    fs::create_dir_all(&root).unwrap();
    write_skill(&root, "beta-skill", "verification tests", "body");
    let bad = root.join("bad-skill");
    fs::create_dir_all(&bad).unwrap();
    fs::write(
        bad.join("SKILL.md"),
        "---\nname: bad-skill\ndescription: [unclosed\n---\nbody\n",
    )
    .unwrap();

    let discovery = discover(&[root]);
    let bad_skill = discovery
        .skills
        .iter()
        .find(|skill| skill.name == "bad-skill")
        .unwrap();
    assert!(bad_skill.parse_error.is_some());

    let graph = build(discovery);
    let bad_recs = recommend(&graph, "bad skill", "", 5);
    assert!(!bad_recs.iter().any(|rec| rec.skill.name == "bad-skill"));
    let recs = recommend(&graph, "verification tests", "", 5);
    assert!(recs.iter().any(|rec| rec.skill.name == "beta-skill"));
    assert!(!recs.iter().any(|rec| rec.skill.name == "bad-skill"));
}

/// #225 B06: `resolved_path`/`resolved_name` semantics are canonicalized:
/// `resolved_name` is set only when the link resolves to a discovered
/// skill; `resolved_path` additionally records the *lexical* resolution
/// of an in-skill-directory `.md` target whether or not the file exists
/// (no body is read — it is a path record, not an existence claim).
#[test]
fn markdown_resolution_distinguishes_lexical_path_from_skill_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    fs::create_dir_all(&root).unwrap();
    write_skill(&root, "alpha-skill", "alpha", "body");
    write_skill(&root, "beta-skill", "beta", "body");

    // Real non-skill file inside the skill dir.
    let alpha = root.join("alpha-skill");
    fs::create_dir_all(alpha.join("references")).unwrap();
    fs::write(alpha.join("references/guide.md"), "guide\n").unwrap();

    fs::write(
        alpha.join("SKILL.md"),
        "---\nname: alpha-skill\ndescription: alpha\n---\n\n\
         [guide](references/guide.md) \
         [missing](references/nope.md) \
         [sib](../beta-skill) \
         [sibmd](../beta-skill/SKILL.md)\n",
    )
    .unwrap();

    let graph = build(discover(&[root]));
    let skill = graph
        .skills
        .iter()
        .find(|s| s.name == "alpha-skill")
        .unwrap();
    let link = |t: &str| skill.links.iter().find(|l| l.target == t).unwrap();

    // Existing non-skill .md: lexical path recorded, no skill name.
    let guide = link("references/guide.md");
    assert!(guide.resolved_name.is_none());
    assert_eq!(
        guide.resolved_path.as_deref(),
        Some(alpha.join("references/guide.md").as_path())
    );

    // Nonexistent in-scope .md: lexical resolution is still recorded —
    // resolved_path is a path record, not an existence or skill claim.
    let missing = link("references/nope.md");
    assert!(missing.resolved_name.is_none());
    assert_eq!(
        missing.resolved_path.as_deref(),
        Some(alpha.join("references/nope.md").as_path())
    );

    // Directory link to a sibling skill resolves to that skill.
    let sib = link("../beta-skill");
    assert_eq!(sib.resolved_name.as_deref(), Some("beta-skill"));
    assert!(sib.resolved_path.is_some());

    // Direct SKILL.md link to a sibling resolves by path.
    let sibmd = link("../beta-skill/SKILL.md");
    assert_eq!(sibmd.resolved_name.as_deref(), Some("beta-skill"));
    assert!(sibmd.resolved_path.is_some());
}
