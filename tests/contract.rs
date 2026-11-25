use serde_json::Value;
use skill_graph::cli::resolve_target;
use skill_graph::discovery::{discover, output};
use skill_graph::resolver::build;
use skill_graph::search::recommend;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/skills")
}

fn write_skill_md(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n"),
    )
    .unwrap();
}

fn assert_skill_summary(value: &Value) {
    for key in ["name", "description", "path", "root", "digest"] {
        assert!(
            value.get(key).and_then(Value::as_str).is_some(),
            "missing {key}"
        );
    }
    let availability = value
        .get("availability")
        .and_then(Value::as_str)
        .expect("availability");
    assert!(matches!(availability, "available" | "parse_error"));
}

#[test]
fn contract_schemas_are_valid_json() {
    for (name, src) in [
        (
            "skill-summary",
            include_str!("../contracts/v1/skill-summary.schema.json"),
        ),
        (
            "discovery-output",
            include_str!("../contracts/v1/discovery-output.schema.json"),
        ),
        (
            "recommendation",
            include_str!("../contracts/v1/recommendation.schema.json"),
        ),
        (
            "resolve-result",
            include_str!("../contracts/v1/resolve-result.schema.json"),
        ),
    ] {
        let value: Value = serde_json::from_str(src).unwrap();
        assert!(
            value["$schema"]
                .as_str()
                .unwrap_or_default()
                .contains("json-schema.org"),
            "{name} schema missing $schema"
        );
    }
}

#[test]
fn discovery_output_matches_contract_shape() {
    let discovery = discover(&[fixture_root()]);
    let value = serde_json::to_value(output(&discovery)).unwrap();
    let skills = value["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 7);
    assert!(value["errors"].as_array().unwrap().is_empty());
    for skill in skills {
        assert_skill_summary(skill);
    }
}

#[test]
fn recommendation_output_matches_contract_shape_and_no_output() {
    let graph = build(discover(&[fixture_root()]));
    let recs = recommend(&graph, "release notes changelog", "version announcement", 3);
    assert!(!recs.is_empty());
    let value = serde_json::to_value(&recs).unwrap();
    for item in value.as_array().expect("recommendation array") {
        assert_skill_summary(&item["skill"]);
        assert!(item["score"].is_number());
        assert!(
            item["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_string)
        );
        assert!(item["observation"].is_string());
    }

    let empty = recommend(&graph, "", "", 3);
    assert!(empty.is_empty());
    assert_eq!(serde_json::to_string(&empty).unwrap(), "[]");
    assert!(recommend(&graph, "quantum gardening", "", 3).is_empty());
}

#[test]
fn resolve_result_contract_covers_name_path_unresolved_and_ambiguity() {
    let graph = build(discover(&[fixture_root()]));
    let name = serde_json::to_value(resolve_target(&graph, "alpha-skill")).unwrap();
    assert_eq!(name["kind"], "name");
    assert_eq!(name["ambiguous"], false);
    assert_eq!(name["skills"].as_array().unwrap().len(), 1);
    assert_skill_summary(&name["skills"][0]);

    let unresolved = serde_json::to_value(resolve_target(&graph, "missing-skill")).unwrap();
    assert_eq!(unresolved["kind"], "unresolved");
    assert_eq!(unresolved["target"], "missing-skill");

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    write_skill_md(&root.join("one"), "dup-skill", "first duplicate");
    write_skill_md(&root.join("two"), "dup-skill", "second duplicate");
    let ambiguous_graph = build(discover(&[root]));
    let ambiguous = serde_json::to_value(resolve_target(&ambiguous_graph, "dup-skill")).unwrap();
    assert_eq!(ambiguous["kind"], "name");
    assert_eq!(ambiguous["ambiguous"], true);
    assert_eq!(ambiguous["skills"].as_array().unwrap().len(), 2);
}
