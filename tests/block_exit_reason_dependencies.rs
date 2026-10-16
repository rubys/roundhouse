//! Callback summaries and nonlocal block exits cannot certify a gradual value flow.

use roundhouse::analyze::{DiagnosticKind, diagnose};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

fn cases() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("block_exit_reason_cases.json")).unwrap()
}

fn source(case: &serde_json::Value, instance: bool, depth: usize) -> String {
    let prefix = if instance { "" } else { "self." };
    let mut source = format!(
        "require \"json\"\nclass Probe\n def {prefix}value(raw)\n  forward0(raw) || \"fallback\"\n end\n"
    );
    for i in 0..depth {
        let body = if i + 1 == depth {
            case["body"].as_str().unwrap().to_owned()
        } else {
            format!("result = forward{}(raw); result", i + 1)
        };
        source.push_str(&format!(" def {prefix}forward{i}(raw)\n  {body}\n end\n"));
    }
    if let Some(support) = case["inline_support"].as_str() {
        source.push_str(support);
    }
    source.push_str("end\n");
    if let Some(support) = case["support"].as_str() {
        source.push_str(support);
    }
    source
}

fn analyze(case: &serde_json::Value, instance: bool, depth: usize) -> (roundhouse::App, String) {
    let source = source(case, instance, depth);
    assert!(
        ruby_prism::parse(source.as_bytes())
            .errors()
            .next()
            .is_none(),
        "{source}"
    );
    let receiver = if instance { "Probe.new" } else { "Probe" };
    let raw = serde_json::to_string(case["raw"].as_str().unwrap()).unwrap();
    let controller = format!(
        "class ProbesController < ActionController::Base\n def show\n  {receiver}.value({raw}).no_such_method\n  head :ok\n end\nend\n"
    );
    let tree = HashMap::from([
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n get \"/probe\", to: \"probes#show\"\nend\n"
                .to_vec(),
        ),
        (
            PathBuf::from("app/controllers/probes_controller.rb"),
            controller.into_bytes(),
        ),
        (PathBuf::from("lib/probe.rb"), source.as_bytes().to_vec()),
    ]);
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    (app, source)
}

fn check_group(group: &str) {
    for case in cases().iter().filter(|case| case["group"] == group) {
        for instance in [false, true] {
            for depth in [2, 4] {
                let (app, source) = analyze(case, instance, depth);
                let files = roundhouse::emit::ruby::emit_library(&app);
                let rbs = files
                    .iter()
                    .filter(|f| f.path.file_name().is_some_and(|n| n == "probe.rbs"))
                    .map(|f| f.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let prefix = if instance { "" } else { "self." };
                let gradual = case["gradual"].as_bool().unwrap();
                let result = if gradual {
                    "untyped"
                } else {
                    case["main_value"].as_str().unwrap()
                };
                let forward = if gradual {
                    "untyped"
                } else {
                    case["main_forward"].as_str().unwrap()
                };
                let mut expected = vec![format!("def {prefix}value: (String raw) -> {result}")];
                expected.extend(
                    (0..depth)
                        .map(|i| format!("def {prefix}forward{i}: (String raw) -> {forward}")),
                );
                expected.sort();
                let mut actual = rbs
                    .lines()
                    .map(str::trim)
                    .filter(|l| {
                        l.starts_with(&format!("def {prefix}value:"))
                            || l.starts_with(&format!("def {prefix}forward"))
                    })
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                actual.sort();
                assert_eq!(actual, expected, "{}\n{source}\n{rbs}", case["id"]);
                if result == "String" {
                    let diagnostics = diagnose(&app);
                    let errors = diagnostics.iter().filter(|d| matches!(&d.kind,
                        DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "no_such_method"))
                        .collect::<Vec<_>>();
                    assert_eq!(errors.len(), 1, "{}\n{source}\n{diagnostics:?}", case["id"]);
                    let file = &app.sources[errors[0].span.file.0 as usize - 1];
                    assert_eq!(file.path, "app/controllers/probes_controller.rb");
                    assert_eq!(file.line_col(errors[0].span.start), (3, 3));
                }
            }
        }
    }
}

#[test]
fn catalog_callback_aliases_and_operands() {
    check_group("catalog");
}
#[test]
fn other_catalog_callback_receivers() {
    check_group("catalog-other");
}
#[test]
fn opaque_catalog_data_remains_gradual() {
    check_group("catalog-data");
}
#[test]
fn yielding_identity_projection_and_extremum_breaks() {
    check_group("break");
}
#[test]
fn literal_and_nested_dispatch_aliases() {
    check_group("alias");
}
#[test]
fn alternate_operands_wrappers_and_block_forms() {
    check_group("operand");
}
#[test]
fn hash_array_and_nil_break_values() {
    check_group("receiver");
}
#[test]
fn nested_break_next_and_ignored_block_controls() {
    check_group("control");
}
#[test]
fn concrete_and_mixed_values_do_not_gain_provenance() {
    check_group("mixed");
}

#[test]
fn safe_navigation_and_boolean_falsy_arms_do_not_admit_data() {
    check_group("safe-navigation");
}

#[test]
fn nonlocal_returns_unwinds_and_local_exit_controls() {
    check_group("nonlocal-exit");
    check_group("local-exit-control");
}

#[test]
fn declared_read_overrides_are_not_provisionally_block_ignoring() {
    check_group("declared-override");
}

#[test]
fn emitted_json_block_exits_and_identity_controls_run() {
    // Run the actual emitted Ruby, in addition to pinning the declarations
    // and routed diagnostic above. No generated Rails fixture is required.
    let bodies = [
        "JSON.parse(raw).tap { break \"ok\" }",
        "JSON.parse(raw).tap { |value, extra=Kernel.raise(\"x\")| true } rescue \"ok\"",
        "JSON.parse(raw).tap { return \"ok\" }",
        "begin; JSON.parse(raw).tap { raise \"x\" }; rescue Exception; \"ok\"; end",
        "JSON.parse(raw).tap { -> { return \"inner\" }.call; \"ignored\" }",
        "JSON.parse(raw).find { break \"ok\" }",
        "JSON.parse(raw).detect { break \"ok\" }",
        "JSON.parse(raw).min { break \"ok\" }",
        "JSON.parse(raw).max { break \"ok\" }",
        "JSON.parse(raw).send(:public_send, \"tap\") { break \"ok\" }",
        "JSON.parse(raw).tap { [1].each { break \"inner\" }; \"ignored\" }",
        "JSON.parse(raw).first { break \"ignored\" }",
    ];
    let cases = cases();
    let selected = cases
        .iter()
        .filter(|c| bodies.contains(&c["body"].as_str().unwrap()))
        .collect::<Vec<_>>();
    for body in bodies {
        assert!(
            selected.iter().any(|case| case["body"] == body),
            "missing runtime witness: {body}"
        );
    }
    for case in selected {
        let (app, original) = analyze(case, false, 2);
        let emitted = roundhouse::emit::ruby::emit_library(&app)
            .into_iter()
            .find(|f| f.path.file_name().is_some_and(|name| name == "probe.rb"))
            .unwrap()
            .content;
        let raw = serde_json::to_string(case["raw"].as_str().unwrap()).unwrap();
        let expected = serde_json::to_string(&case["expected_value"].to_string()).unwrap();
        for (mode, program) in [("source", original), ("emitted", emitted)] {
            let script = format!(
                "require 'json'\n{program}\nvalue = Probe.value({raw})\nraise 'wrong value' unless JSON.generate(value) == {expected}\nbegin\n value.no_such_method\n raise 'bad call accepted'\nrescue NoMethodError\nend\nputs 'ok'\n"
            );
            let mut child = Command::new("ruby")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Ruby is required for emitted regressions");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{} {mode}: {}",
                case["id"],
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                "ok",
                "{} {mode}",
                case["id"]
            );
        }
    }
}
