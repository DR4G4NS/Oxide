//! Self-test suite for dashmap_guard.
//!
//! Every fixture is exercised independently with explicit expected codes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_dashmap_guard")
}

fn run_paths(paths: &str, extra: &[&str]) -> (i32, String) {
    let mut cmd = Command::new(bin());
    cmd.arg("check").arg(FIXTURES).arg("--paths").arg(paths);
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().expect("spawn dashmap_guard");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

fn run_single(rel: &str, extra: &[&str]) -> (i32, String) {
    let paths = format!("{rel},shared_types.rs");
    run_paths(&paths, extra)
}

fn list_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn rel(p: &Path) -> String {
    p.strip_prefix(FIXTURES)
        .expect("under fixtures")
        .to_string_lossy()
        .to_string()
}

fn must_fail_expectations() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        ("must_fail/dm001_get_insert.rs", vec!["DM001"]),
        ("must_fail/dm001_get_get_mut.rs", vec!["DM001"]),
        ("must_fail/dm001_try_operator.rs", vec!["DM001"]),
        ("must_fail/dm001_arc_field.rs", vec!["DM001"]),
        ("must_fail/dm001_alias_arc_field.rs", vec!["DM001"]),
        ("must_fail/dm001_alias_declared_after_field.rs", vec!["DM001"]),
        ("must_fail/dm001_alias_cross_module.rs", vec!["DM001"]),
        ("must_fail/dm002_get_mut_get.rs", vec!["DM002"]),
        ("must_fail/dm002_get_mut_insert.rs", vec!["DM002"]),
        ("must_fail/dm002_entry_or_default.rs", vec!["DM002"]),
        ("must_fail/dm003_iter_helper_remove.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_next_insert.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_nth_insert.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_find_insert.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_any_insert.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_next_chunk.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_mut_helper_read.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_mut_helper_mut.rs", vec!["DM003"]),
        ("must_fail/dm003_iter_refs_collect.rs", vec!["DM003"]),
        ("must_fail/dm004_get_transitive_helper.rs", vec!["DM004"]),
        ("must_fail/dm004_dashmap_param.rs", vec!["DM004"]),
        ("must_fail/dm004_renamed_param.rs", vec!["DM004"]),
        ("must_fail/dm004_type_aware_false_recursion.rs", vec!["DM004"]),
        ("must_fail/type_aware_two_modules.rs", vec!["DM004"]),
        ("must_fail/dm005_guard_across_await.rs", vec!["DM005"]),
        ("must_fail/dm900_trait_build.rs", vec!["DM900"]),
        ("must_fail/dm900_trait_config.rs", vec!["DM900"]),
        ("must_fail/dm900_trait_copy.rs", vec!["DM900"]),
        ("must_fail/dm900_trait_power.rs", vec!["DM900"]),
        ("must_fail/dm900_unresolved_trait_call.rs", vec!["DM900"]),
        ("must_fail/dm900_nested_unresolved.rs", vec!["DM900"]),
        ("must_fail/alias_map_mutation.rs", vec!["DM001"]),
        ("must_fail/iflet_guard_mutation.rs", vec!["DM001"]),
        ("must_fail/match_guard_helper.rs", vec!["DM004"]),
    ])
}

#[test]
fn each_must_fail_fixture_independently() {
    let expectations = must_fail_expectations();
    for path in list_rs(&Path::new(FIXTURES).join("must_fail")) {
        let key = rel(&path);
        let codes = expectations
            .get(key.as_str())
            .unwrap_or_else(|| panic!("missing expectation table entry for {key}"));
        let deny = codes.iter().any(|c| c.starts_with("DM900"));
        let (code, out) = if deny {
            run_single(&key, &["--deny-warnings"])
        } else {
            run_single(&key, &[])
        };
        assert_ne!(code, 0, "{key} should exit non-zero\n{out}");
        for expected in codes {
            assert!(
                out.contains(expected),
                "{key} should contain {expected}\n{out}"
            );
        }
    }
}

#[test]
fn each_must_pass_fixture_independently() {
    for path in list_rs(&Path::new(FIXTURES).join("must_pass")) {
        let key = rel(&path);
        let (code, out) = run_single(&key, &[]);
        assert_eq!(code, 0, "{key} must exit 0\n{out}");
        assert!(
            out.contains("0 error(s)"),
            "{key} must have zero errors\n{out}"
        );
    }
}

#[test]
fn parse_error_fixture_fails_closed_tool002() {
    let (code, out) = run_single("malformed/parse_error.rs", &[]);
    assert_ne!(code, 0, "parse error must block\n{out}");
    assert!(out.contains("TOOL002"), "expected TOOL002\n{out}");
}

#[test]
fn historical_unsafe_is_dm004_and_corrected_passes() {
    let (code, out) = run_paths("historical,shared_types.rs", &[]);
    assert_ne!(code, 0, "historical (unsafe) must exit non-zero\n{out}");
    assert!(out.contains("DM004"), "historical must produce DM004\n{out}");
    let (code2, out2) = run_single("historical/corrected_fixture.rs", &[]);
    assert_eq!(code2, 0, "corrected historical form must pass\n{out2}");
}

#[test]
fn narrow_suppression_is_honored_and_surfaces_in_json() {
    let (code, out) = run_paths("suppression,shared_types.rs", &[]);
    assert_eq!(code, 0, "a valid suppression must not block\n{out}");
    assert!(out.contains("(suppressed:"), "suppression reason must surface\n{out}");
    let (_, jout) = run_paths("suppression,shared_types.rs", &["--format", "json"]);
    assert!(jout.contains("\"suppressed\": 1"), "JSON should count suppression\n{jout}");
}

#[test]
fn malformed_suppressions_warn_but_do_not_block() {
    let (code, out) = run_single("malformed/malformed_suppression.rs", &[]);
    assert_eq!(code, 0, "malformed suppressions must not block\n{out}");
    assert!(out.contains("DM901"), "malformed suppression must surface as DM901\n{out}");
}

#[test]
fn output_is_deterministic_and_sorted() {
    let (_, a) = run_paths("must_fail,shared_types.rs", &[]);
    let (_, b) = run_paths("must_fail,shared_types.rs", &[]);
    assert_eq!(a, b, "text output must be deterministic");
}

#[test]
fn type_aware_false_recursion_resolves_admin_not_self() {
    let (code, out) = run_single("must_fail/dm004_type_aware_false_recursion.rs", &[]);
    assert_ne!(code, 0, "expected blocking diagnostic\n{out}");
    assert!(out.contains("DM004"), "{out}");
    assert!(
        !out.contains("Console::set_value -> Console::set_value"),
        "must not self-recurse\n{out}"
    );
}
