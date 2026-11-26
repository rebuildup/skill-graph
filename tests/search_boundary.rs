// Regression tests for audit findings D-03/I-02 (issue #170, review #168):
// ASCII terms must match on word boundaries (no "pr" -> "improve"/"project",
// no "log" -> "changelog", no "test" -> "latest"), abbreviations like "pr"
// must still reach standalone "PR" tokens, hyphenated names must still match
// multi-word queries, CJK bigram matching must be unchanged, and admission
// must not flip when a second same-domain skill enters the corpus.

use skill_graph::discovery::discover;
use skill_graph::resolver::build;
use skill_graph::search::recommend;
use std::fs;
use std::path::Path;

fn write_skill_md(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n"),
    )
    .unwrap();
}

fn names(recs: &[skill_graph::model::Recommendation]) -> Vec<&str> {
    recs.iter().map(|r| r.skill.name.as_str()).collect()
}

#[test]
fn pull_request_pr_does_not_hit_project_or_improve() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    // "project"/"improving" contain the substring "pr" but never the token.
    write_skill_md(
        &root.join("project-helper"),
        "project-helper",
        "Use for project management and improving workflow documentation.",
    );
    // Legitimate abbreviation use: standalone "PR" token plus spelled-out
    // "pull request" in the description.
    write_skill_md(
        &root.join("pr-writer"),
        "pr-writer",
        "Use for PR reviews, pull request approvals, and merge workflows.",
    );
    let graph = build(discover(&[root]));

    let recs = recommend(&graph, "pull request", "", 5);
    let got = names(&recs);
    assert!(
        got.contains(&"pr-writer"),
        "expected pr-writer via PR/pull/request tokens, got: {got:?}"
    );
    assert!(
        !got.contains(&"project-helper"),
        "\"pr\" must not substring-match \"project\"/\"improving\", got: {got:?}"
    );
}

#[test]
fn ascii_terms_do_not_match_inside_longer_words() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(
        &root.join("changelog-keeper"),
        "changelog-keeper",
        "Use for compiling the project changelog before releases.",
    );
    write_skill_md(
        &root.join("latest-sync"),
        "latest-sync",
        "Use for syncing the latest dependency versions.",
    );
    let graph = build(discover(&[root]));

    // "log" is a substring of "changelog" and "test" a substring of
    // "latest"; neither is a word-boundary match.
    for query in ["log", "test"] {
        let recs = recommend(&graph, query, "", 5);
        assert!(
            recs.is_empty(),
            "query {query:?} must not hit changelog/latest substrings, got: {:?}",
            names(&recs)
        );
    }
}

#[test]
fn hyphenated_skill_name_matches_split_query_words() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(
        &root.join("code-naming"),
        "code-naming",
        "Use for naming and rename design of code identifiers.",
    );
    let graph = build(discover(&[root]));

    let recs = recommend(&graph, "code naming", "", 5);
    assert_eq!(names(&recs), vec!["code-naming"]);
}

#[test]
fn two_same_domain_skills_are_both_recommendable() {
    // Before #170, df(term) == 1 admission meant adding a second same-domain
    // skill (df 1 -> 2) made both vanish from recommendations.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(
        &root.join("deploy-web"),
        "deploy-web",
        "Use for deployment rollback of web services.",
    );
    write_skill_md(
        &root.join("deploy-api"),
        "deploy-api",
        "Use for deployment rollback of api services.",
    );
    write_skill_md(
        &root.join("unrelated"),
        "unrelated",
        "Use for sourdough baking schedules.",
    );
    let graph = build(discover(&[root]));

    let recs = recommend(&graph, "deployment rollback procedure", "", 5);
    let got = names(&recs);
    assert!(got.contains(&"deploy-web"), "got: {got:?}");
    assert!(got.contains(&"deploy-api"), "got: {got:?}");
    assert!(!got.contains(&"unrelated"), "got: {got:?}");
}

#[test]
fn cjk_bigrams_still_match_descriptions() {
    // CJK terms are bigrams with no word boundaries; they must keep plain
    // substring semantics (#61) alongside the new ASCII boundary rule.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(
        &root.join("jp-ops"),
        "jp-ops",
        "デプロイとロールバックの手順を扱う。",
    );
    let graph = build(discover(&[root]));

    let recs = recommend(&graph, "ロールバック", "", 5);
    assert_eq!(names(&recs), vec!["jp-ops"]);
}
