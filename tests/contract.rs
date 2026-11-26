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
        (
            "skill-usage-event",
            include_str!("../contracts/v1/skill-usage-event.schema.json"),
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
    assert_eq!(skills.len(), 8);
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

#[test]
fn resolve_by_unnormalized_absolute_path_matches() {
    let graph = build(discover(&[fixture_root()]));
    let skill_path = graph.skills[0].path.clone();
    // Detour through `<dir>/../<dir>` which normalizes back to the same
    // file; by_path lookup must still hit.
    let dir = skill_path.parent().unwrap();
    let detoured = dir
        .parent()
        .unwrap()
        .join(dir.file_name().unwrap())
        .join("..")
        .join(dir.file_name().unwrap())
        .join(skill_path.file_name().unwrap());
    let result = serde_json::to_value(resolve_target(&graph, detoured.to_str().unwrap())).unwrap();
    assert_eq!(result["kind"], "path", "{result}");
    assert_eq!(result["skill"]["name"], graph.skills[0].name);
}

// --- JSON Schema validation against contracts/v1/*.schema.json ---
// The skill-* schemas reference skill-summary.schema.json by relative
// $ref; the registry resolves it to the published $id base.

const SKILL_SUMMARY_URI: &str = "https://skill-graph.local/contracts/v1/skill-summary.schema.json";

fn validator_for(schema_src: &str, label: &str) -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(schema_src).unwrap();
    let summary: Value =
        serde_json::from_str(include_str!("../contracts/v1/skill-summary.schema.json")).unwrap();
    let registry = jsonschema::Registry::new()
        .add(SKILL_SUMMARY_URI, summary)
        .expect("registry add")
        .prepare()
        .expect("registry prepare");
    jsonschema::options()
        .with_registry(&registry)
        .build(&schema)
        .unwrap_or_else(|e| panic!("{label}: schema compile failed: {e}"))
}

fn assert_schema_valid(schema_src: &str, instance: &Value, label: &str) {
    let validator = validator_for(schema_src, label);
    if let Err(errors) = validator.validate(instance) {
        panic!("{label}: instance violates schema: {errors}");
    }
}

fn assert_schema_invalid(schema_src: &str, instance: &Value, label: &str) {
    let validator = validator_for(schema_src, label);
    assert!(
        validator.validate(instance).is_err(),
        "{label}: invalid instance unexpectedly passed schema"
    );
}

#[test]
fn contract_outputs_validate_against_schemas() {
    let discovery = serde_json::to_value(output(&discover(&[fixture_root()]))).unwrap();
    assert_schema_valid(
        include_str!("../contracts/v1/discovery-output.schema.json"),
        &discovery,
        "discovery-output",
    );

    let graph = build(discover(&[fixture_root()]));
    let recs = serde_json::to_value(recommend(&graph, "release notes changelog", "", 3)).unwrap();
    assert_schema_valid(
        include_str!("../contracts/v1/recommendation.schema.json"),
        &recs,
        "recommendation",
    );

    let resolved = serde_json::to_value(resolve_target(&graph, "alpha-skill")).unwrap();
    assert_schema_valid(
        include_str!("../contracts/v1/resolve-result.schema.json"),
        &resolved,
        "resolve-result",
    );
}

#[test]
fn skill_usage_events_validate_against_event_schema() {
    use skill_graph::events::{
        Confidence, Detection, ObservedVia, RecommendedSkill, Runtime, SkillUsageEvent, Trigger,
    };
    let schema = include_str!("../contracts/v1/skill-usage-event.schema.json");
    let events = [
        SkillUsageEvent::prompt_submit(
            Runtime::ClaudeCode,
            ObservedVia::Hook,
            "s1",
            "2026-01-01T00:00:00Z",
            "hello",
        ),
        SkillUsageEvent::recommendation(
            Runtime::Codex,
            ObservedVia::Plugin,
            "s1",
            "2026-01-01T00:00:01Z",
            None,
            Trigger::SkillLoad,
            vec![RecommendedSkill {
                name: "alpha-skill".into(),
                digest: Some("abc".into()),
            }],
            true,
            "recommended",
            Some(12),
        ),
        SkillUsageEvent::skill_load(
            Runtime::Opencode,
            ObservedVia::Transcript,
            "s2",
            "2026-01-01T00:00:02Z",
            Some("Read".into()),
            Some("beta-skill".into()),
            Some("/x/SKILL.md".into()),
            Detection::FileRead,
            Confidence::High,
        ),
    ];
    for e in &events {
        let v = serde_json::to_value(e).unwrap();
        assert_schema_valid(schema, &v, "skill-usage-event");
    }
}

#[test]
fn contract_schemas_reject_invalid_instances() {
    use serde_json::json;
    let event_schema = include_str!("../contracts/v1/skill-usage-event.schema.json");

    // Missing a required field.
    assert_schema_invalid(
        event_schema,
        &json!({"schema_version": 1, "kind": "skill_load", "runtime": "codex"}),
        "event-missing-observed_via",
    );
    // Enum violation.
    assert_schema_invalid(
        event_schema,
        &json!({
            "schema_version": 1, "kind": "bogus", "runtime": "codex",
            "observed_via": "hook"
        }),
        "event-bad-kind",
    );
    // Conditional requirement: prompt_submit must carry prompt_hash.
    assert_schema_invalid(
        event_schema,
        &json!({
            "schema_version": 1, "kind": "prompt_submit", "runtime": "codex",
            "observed_via": "hook"
        }),
        "event-prompt-submit-without-hash",
    );
    // recommendation must carry recommended/emitted/reason/trigger.
    assert_schema_invalid(
        event_schema,
        &json!({
            "schema_version": 1, "kind": "recommendation", "runtime": "codex",
            "observed_via": "hook"
        }),
        "event-recommendation-missing-fields",
    );

    // recommendation item missing required members.
    assert_schema_invalid(
        include_str!("../contracts/v1/recommendation.schema.json"),
        &json!([{"skill": {"name": "x"}}]),
        "recommendation-missing-score",
    );

    // discovery output must be an object with skills+errors arrays.
    assert_schema_invalid(
        include_str!("../contracts/v1/discovery-output.schema.json"),
        &json!({"skills": "not-an-array", "errors": []}),
        "discovery-skills-not-array",
    );

    // resolve-result: name kind requires ambiguous+skills.
    assert_schema_invalid(
        include_str!("../contracts/v1/resolve-result.schema.json"),
        &json!({"kind": "name", "skills": []}),
        "resolve-name-missing-ambiguous",
    );
}

#[test]
fn plugin_manifest_version_matches_crate() {
    // plugins/skill-graph is bundled verbatim into the npm package; its
    // manifest version must track the product release (spec §distribution).
    let manifest: Value =
        serde_json::from_str(include_str!("../plugins/skill-graph/plugin.json")).unwrap();
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
}

/// The OpenCode plugin hand-constructs event objects in TypeScript and
/// appends them to the shared JSONL log, bypassing every Rust emit path.
/// Guard the contract: every top-level key written inside `emit({...})`
/// literals (and the `base()` helper) must be a declared schema property,
/// so a future edit cannot ship `additionalProperties: false` violations
/// that `observe` would silently drop.
#[test]
fn opencode_plugin_emit_shapes_stay_within_event_schema() {
    let src = include_str!("../src/assets/opencode-plugin.ts");
    let schema_src = include_str!("../contracts/v1/skill-usage-event.schema.json");
    let schema: Value = serde_json::from_str(schema_src).unwrap();
    let allowed: std::collections::HashSet<String> = schema["properties"]
        .as_object()
        .expect("event schema properties")
        .keys()
        .cloned()
        .collect();

    // Brace-match each `emit({...})` literal and the object returned by
    // base(); collect `ident:` keys at depth 1 of each.
    let mut opens: Vec<usize> = src
        .match_indices("emit({")
        .map(|(i, _)| i + "emit(".len())
        .collect();
    if let Some(f) = src.find("function base")
        && let Some(r) = src[f..].find("return {")
    {
        opens.push(f + r + "return ".len());
    }
    assert!(!opens.is_empty(), "no emit() literals found in plugin.ts");

    let bytes = src.as_bytes();
    let mut keys = std::collections::HashSet::new();
    for open in opens {
        // `open` is the byte offset of the object's `{`.
        let mut depth = 0i32;
        let mut j = open;
        let mut in_str = false;
        let mut str_ch = b'\0';
        while j < bytes.len() {
            let c = bytes[j];
            if in_str {
                if c == b'\\' {
                    j += 2;
                    continue;
                }
                if c == str_ch {
                    in_str = false;
                }
            } else if c == b'"' || c == b'\'' || c == b'`' {
                in_str = true;
                str_ch = c;
            } else if c == b'{' {
                depth += 1;
            } else if c == b'}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        let body = &src[open + 1..j];
        // Top-level `ident:` keys (depth 1 inside this object).
        let bb = body.as_bytes();
        let mut d = 1i32;
        let mut k = 0usize;
        while k < bb.len() {
            let c = bb[k];
            if c == b'"' || c == b'\'' || c == b'`' {
                let q = c;
                k += 1;
                while k < bb.len() && bb[k] != q {
                    k += if bb[k] == b'\\' { 2 } else { 1 };
                }
            } else if c == b'{' || c == b'[' || c == b'(' {
                d += 1;
            } else if c == b'}' || c == b']' || c == b')' {
                d -= 1;
            } else if d == 1 && (c.is_ascii_alphabetic() || c == b'_') {
                // Byte offsets inside `body` may not be char boundaries;
                // accumulate the ident on bytes instead.
                let start = k;
                while k < bb.len() && (bb[k].is_ascii_alphanumeric() || bb[k] == b'_') {
                    k += 1;
                }
                let ident = std::str::from_utf8(&bb[start..k]).unwrap();
                if body[k..].trim_start().starts_with(':') {
                    keys.insert(ident.to_string());
                }
                continue;
            }
            k += 1;
        }
    }

    for key in &keys {
        assert!(
            allowed.contains(key),
            "plugin emits key `{key}` not declared in skill-usage-event schema"
        );
    }
    // base() uses a `kind` shorthand (no colon); the fixed base fields
    // must all be present.
    assert!(src.contains("kind,"), "base() must set kind");
    for f in ["schema_version", "runtime", "observed_via", "timestamp"] {
        assert!(keys.contains(f), "base() field `{f}` not found");
    }
}
