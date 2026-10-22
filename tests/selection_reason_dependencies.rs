//! Fail-closed transfers preserve pinned main declarations outside the whitelist.

use std::{collections::HashMap, path::PathBuf};

use roundhouse::analyze::{DiagnosticKind, diagnose};

fn check(body: &str, instance: bool, depth: usize, gradual: bool, _forward: &str) {
    let prefix = if instance { "" } else { "self." };
    let receiver = if instance { "Probe.new" } else { "Probe" };
    let fallback = if gradual { ":fallback" } else { "\"fallback\"" };
    let mut source =
        format!("class Probe\n def {prefix}value(raw)\n forward0(raw) || {fallback}\n end\n");
    for i in 0..depth {
        let expression = if i + 1 == depth {
            body.to_owned()
        } else {
            format!("result = forward{}(raw); result", i + 1)
        };
        source.push_str(&format!(
            "def {prefix}forward{i}(raw)\n {expression}\n end\n"
        ));
    }
    source.push_str("end\n");
    if body.contains("Dig") {
        source.push_str(
            r###"class ValueDig
 def self.dig(value); value; end
 def dig(value); value; end
 def self.try(value); value; end
 def try(value); value; end
end
class ConstantDig
 def self.dig(value); "ok"; end
 def dig(value); "ok"; end
 def self.try(value); "ok"; end
 def try(value); "ok"; end
end
class ChildDig < ValueDig
end
module DigValue
 def dig(value); value; end
end
class IncludedDig
 include DigValue
end
"###,
        );
    }
    assert!(
        ruby_prism::parse(source.as_bytes())
            .errors()
            .next()
            .is_none(),
        "{source}"
    );
    let controller = format!(
        "class ProbesController < ActionController::Base\n def show\n  {receiver}.value(\"42\").no_such_method\n  head :ok\n end\nend\n"
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
    let rbs = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let baselines: serde_json::Value = serde_json::from_str(include_str!("selection_reason_main.json")).unwrap();
    let baseline = &baselines[body];
    assert!(!baseline.is_null(), "missing pinned-main case: {body}");
    // This receiver-only JSON projection is the one intentional #964 repair
    // in this collection. Concrete/mixed receivers retain main's answer.
    let result = if body == r###"JSON.parse(raw).dig(JSON.parse("0"))"### {
        "untyped"
    } else { baseline["value"].as_str().unwrap() };
    let forward = baseline["forward"].as_str().unwrap();
    let mut expected = vec![format!("def {prefix}value: (String raw) -> {result}")];
    expected.extend((0..depth).map(|i| format!("def {prefix}forward{i}: (String raw) -> {forward}")));
    let actual: Vec<_> = rbs
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with(&format!("def {prefix}value:"))
                || line.starts_with(&format!("def {prefix}forward"))
        })
        .map(str::to_owned)
        .collect();
    expected.sort();
    let mut actual = actual;
    actual.sort();
    assert_eq!(actual, expected, "{source}\n{rbs}");

    if result == "String" || result == "Symbol" {
        let diagnostics = diagnose(&app);
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    &d.kind, DiagnosticKind::SendDispatchFailed { method, .. }
                        if method.as_str() == "no_such_method"
                )
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "lost real dispatch error: {source}\n{diagnostics:?}"
        );
        let d = errors[0];
        let file = &app.sources[d.span.file.0 as usize - 1];
        assert_eq!(file.path, "app/controllers/probes_controller.rb");
        assert_eq!(file.line_col(d.span.start), (3, 3));
    }
}

#[test]
fn six_review_witnesses() {
    for (body, gradual, forward) in [
        (r###"["ok"].dig(JSON.parse("0"))"###, false, "untyped"),
        (
            r###"{"k" => "ok"}.dig(JSON.parse('"k"'))"###,
            false,
            "untyped",
        ),
        (r###"["ok"].dig(0) { JSON.parse(raw) }"###, false, "untyped"),
        (
            r###"["ok"].as_json { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.as_json { JSON.parse(raw) }["k"]"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.as_json(JSON.parse("{}"))["k"]"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn dig_receiver_shapes_and_hops() {
    for (body, gradual, forward) in [
        (
            r###"[{"k" => "ok"}].dig(JSON.parse("0"), JSON.parse('"k"'))"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => ["ok"]}.dig(JSON.parse('"k"'), JSON.parse("0"))"###,
            false,
            "untyped",
        ),
        (
            r###"[{"k" => ["ok"]}].dig(JSON.parse("0"), JSON.parse('"k"'), JSON.parse("0"))"###,
            false,
            "untyped",
        ),
        (
            r###"{JSON.parse('"k"') => "ok"}.dig("k")"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => {JSON.parse('"nested"') => "ok"}}.dig("k", "nested")"###,
            false,
            "untyped",
        ),
        (r###"["ok", 1].dig(JSON.parse("0"))"###, false, "untyped"),
        (
            r###"items = ["ok"]; index = JSON.parse("0"); items.dig(index)"###,
            false,
            "untyped",
        ),
        (
            r###"items = {"k" => "ok"}; key = JSON.parse('"k"'); items.dig(key)"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"]&.dig(JSON.parse("0")) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"[["ok"], nil].first&.dig(JSON.parse("0"))"###,
            false,
            "untyped",
        ),
        (
            r###"[["ok"], {"k" => "ok"}].first.dig(JSON.parse("0"))"###,
            false,
            "untyped",
        ),
        (
            r###"[{"k" => "ok"}, ["ok"]].first.dig(JSON.parse('"k"'))"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.with_indifferent_access.dig(JSON.parse('"k"'))"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn as_json_options_and_aliases() {
    for (body, gradual, forward) in [
        (
            r###"["ok"].as_json(JSON.parse("{}")) { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.as_json(only: JSON.parse('["k"]'))["k"]"###,
            false,
            "untyped",
        ),
        (
            r###"options = JSON.parse("{}"); items = {"k" => "ok"}; items.as_json(options)["k"]"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"]&.as_json(JSON.parse("{}")) { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
        (
            r###"[["ok"], nil].first&.as_json { JSON.parse(raw) }&.first"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].send(:as_json, JSON.parse("{}")) { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.public_send("as_json", JSON.parse("{}")) { JSON.parse(raw) }["k"]"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].__send__(:send, :as_json, JSON.parse("{}")) { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn dig_reflective_aliases() {
    for (body, gradual, forward) in [
        (
            r###"["ok"].try(:dig, JSON.parse("0")) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].try!(:dig, JSON.parse("0")) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].send(:dig, JSON.parse("0")) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.public_send("dig", JSON.parse('"k"')) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].__send__(:send, :dig, JSON.parse("0")) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn fetch_and_selector_projections() {
    for (body, gradual, forward) in [
        (r###"["ok"].fetch(JSON.parse("0"))"###, false, "String"),
        (
            r###"["ok"].fetch(JSON.parse("0"), "fallback")"###,
            false,
            "String",
        ),
        (
            r###"["ok"].fetch(JSON.parse("0"), JSON.parse(raw)) { "ok" }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].send(:fetch, JSON.parse("0"), JSON.parse(raw)) { "ok" }"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.fetch(JSON.parse('"k"'))"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.fetch(JSON.parse('"k"'), "fallback")"###,
            false,
            "String",
        ),
        (r###"["ok"][JSON.parse("0")]"###, false, "String?"),
        (r###"{"k" => "ok"}[JSON.parse('"k"')]"###, false, "String?"),
        (
            r###"{"k" => "ok"}.values_at(JSON.parse('"k"')).first"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.fetch_values(JSON.parse('"k"')).first"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.slice(JSON.parse('"k"'))["k"]"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.except(JSON.parse('"absent"'))["k"]"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.without(JSON.parse('"absent"'))["k"]"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.with_indifferent_access.values_at(JSON.parse('"k"')).first"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.with_indifferent_access.slice(JSON.parse('"k"'))["k"]"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].sample(random: JSON.parse("{}"))"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].take(JSON.parse("1")).first"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].drop(JSON.parse("0")).first"###,
            false,
            "String?",
        ),
        (r###"[{"k" => "ok"}].pluck("k").first"###, false, "String?"),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn yielded_parameter_dependencies() {
    for (body, gradual, forward) in [
        (
            r###"["ok"].try(:fetch, 0, JSON.parse(raw)) { |index| index }"###,
            false,
            "untyped",
        ),
        (
            r###"Probe.class_exec("ok", JSON.parse(raw)) { |value, other| value }"###,
            false,
            "untyped",
        ),
        (
            r###"Probe.module_exec("ok", JSON.parse(raw)) { |value, other| value }"###,
            false,
            "untyped",
        ),
        (
            r###"Probe.class_eval { |receiver| "ok" }"###,
            false,
            "String",
        ),
        (
            r###"Probe.module_eval { |receiver| "ok" }"###,
            false,
            "String",
        ),
        (
            r###"["ok"].fetch(0, JSON.parse(raw)) { |index| index }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].send(:fetch, 0, JSON.parse(raw)) { |index| index }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].public_send("fetch", 0, JSON.parse(raw)) { |index| index }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].__send__(:send, :fetch, 0, JSON.parse(raw)) { |index| index }"###,
            false,
            "untyped",
        ),
        (
            r###"Object.new.instance_exec("ok", JSON.parse(raw)) { |value, other| value }"###,
            false,
            "untyped",
        ),
        (
            r###"Object.new.send(:instance_exec, "ok", JSON.parse(raw)) { |value, other| value }"###,
            false,
            "untyped",
        ),
        (
            r###"Object.new.class.instance_exec("ok", JSON.parse(raw)) { |value, other| value }"###,
            false,
            "untyped",
        ),
        (
            r###"Time.use_zone(JSON.parse('"UTC"')) { |unused| unused }"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn ignored_predicate_and_iteration_results() {
    for (body, gradual, forward) in [
        (r###"nil.try { JSON.parse(raw) }"###, false, "untyped"),
        (r###"nil.try! { JSON.parse(raw) }"###, false, "untyped"),
        (
            r###"nil.try(:dig, JSON.parse(raw)) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"nil.send(:try!, :dig, JSON.parse(raw)) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].select { JSON.parse(raw) }.first"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].reject { JSON.parse("false") }.first"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].sort_by { JSON.parse("0") }.first"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].each { JSON.parse(raw) }.first"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.select { JSON.parse(raw) }["k"]"###,
            false,
            "String?",
        ),
        (
            r###"{"k" => "ok"}.each_value { JSON.parse(raw) }["k"]"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].tap { JSON.parse(raw) }.first"###,
            false,
            "String?",
        ),
        (
            r###"["ok"].presence { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn constructors_and_symbolic_folds() {
    for (body, gradual, forward) in [
        (
            r###"Array.[]("ok") { JSON.parse(raw) }.first"###,
            false,
            "untyped",
        ),
        (
            r###"Hash.[]("k", "ok") { JSON.parse(raw) }["k"]"###,
            false,
            "untyped",
        ),
        (r###"["ok"].reduce(JSON.parse('"+"'))"###, false, "untyped"),
        (
            r###"["ok"].inject("", JSON.parse('"+"')) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"{"k" => "ok"}.reduce("", JSON.parse('"+"')) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"Object.new.instance_variable_get(:@x) { JSON.parse(raw) }"###,
            false,
            "untyped",
        ),
        (
            r###"["ok"].try(JSON.parse('"dig"'), 0)"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn declared_projection_returns() {
    for (body, gradual, forward) in [
        (
            r###"ConstantDig.__send__(:send, :dig, JSON.parse(raw))"###,
            false,
            "untyped",
        ),
        (
            r###"ConstantDig.send(:try, JSON.parse(raw))"###,
            false,
            "untyped",
        ),
        (r###"ConstantDig.dig(JSON.parse(raw))"###, false, "untyped"),
        (
            r###"ConstantDig.new.dig(JSON.parse(raw))"###,
            false,
            "untyped",
        ),
        (
            r###"ConstantDig.send(:dig, JSON.parse(raw))"###,
            false,
            "untyped",
        ),
        (
            r###"ConstantDig.new.public_send("dig", JSON.parse(raw))"###,
            false,
            "untyped",
        ),
        (r###"ConstantDig.try(JSON.parse(raw))"###, false, "untyped"),
        (
            r###"ConstantDig.new.try(JSON.parse(raw))"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn source_values_only_survive_whitelisted_value_flows() {
    for (body, gradual, forward) in [
        (
            r###"ValueDig.__send__(:send, :dig, JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"ValueDig.send(:try, JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"ValueDig.new.__send__(:send, :try, JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"IncludedDig.new.__send__(:send, :dig, JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (r###"ValueDig.dig(JSON.parse(raw))"###, true, "untyped"),
        (r###"ValueDig.new.dig(JSON.parse(raw))"###, true, "untyped"),
        (
            r###"ValueDig.send(:dig, JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"ValueDig.new.public_send("dig", JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (r###"ChildDig.dig(JSON.parse(raw))"###, true, "untyped"),
        (r###"ChildDig.new.dig(JSON.parse(raw))"###, true, "untyped"),
        (
            r###"IncludedDig.new.dig(JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (r###"ValueDig.try(JSON.parse(raw))"###, true, "untyped"),
        (r###"ValueDig.new.try(JSON.parse(raw))"###, true, "untyped"),
        (
            r###"[ValueDig.new, nil].first&.dig(JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"[ValueDig.new, ConstantDig.new].first.dig(JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"[ConstantDig.new, ValueDig.new].first.dig(JSON.parse(raw))"###,
            true,
            "untyped",
        ),
        (
            r###"Object.new.instance_exec(JSON.parse(raw), "ok") { |value, other| value }"###,
            true,
            "untyped",
        ),
        (
            r###"Object.new.instance_exec("ok", JSON.parse(raw)) { |value, other| other }"###,
            true,
            "untyped",
        ),
        (
            r###"["ok"].fetch(JSON.parse("2"), "unused") { |index| index }"###,
            true,
            "untyped",
        ),
        (
            r###"Timeout.timeout(JSON.parse("1")) { |seconds| seconds }"###,
            true,
            "untyped",
        ),
        (
            r###"Probe.class_exec(JSON.parse(raw), "ok") { |value, other| value }"###,
            true,
            "untyped",
        ),
        (
            r###"Probe.module_exec("ok", JSON.parse(raw)) { |value, other| other }"###,
            true,
            "untyped",
        ),
        (r###"[JSON.parse(raw)].dig(0)"###, true, "untyped"),
        (r###"{"k" => JSON.parse(raw)}.dig("k")"###, true, "untyped"),
        (
            r###"{"k" => [JSON.parse(raw)]}.dig("k", 0)"###,
            true,
            "untyped",
        ),
        (
            r###"[{"k" => JSON.parse(raw)}].dig(0, "k")"###,
            true,
            "untyped",
        ),
        (
            r###"JSON.parse(raw).dig(JSON.parse("0"))"###,
            true,
            "untyped",
        ),
        (r###"[JSON.parse(raw)]&.dig(0) { "ok" }"###, true, "untyped"),
        (r###"[JSON.parse(raw)].send(:dig, 0)"###, true, "untyped"),
        (
            r###"{"k" => JSON.parse(raw)}.public_send("as_json", JSON.parse("{}"))["k"]"###,
            true,
            "untyped",
        ),
        (
            r###"[JSON.parse(raw)].as_json(JSON.parse("{}")) { "ok" }.first"###,
            true,
            "untyped",
        ),
        (
            r###"{"k" => JSON.parse(raw)}.as_json { "ok" }["k"]"###,
            true,
            "untyped",
        ),
        (
            r###"["ok"].fetch(1, "unused") { JSON.parse(raw) }"###,
            true,
            "untyped",
        ),
        (
            r###"[JSON.parse(raw)].fetch(JSON.parse("0"))"###,
            true,
            "untyped",
        ),
        (
            r###"{"k" => JSON.parse(raw)}.values_at(JSON.parse('"k"')).first"###,
            true,
            "untyped",
        ),
        (
            r###"{"k" => JSON.parse(raw)}.slice(JSON.parse('"k"'))["k"]"###,
            true,
            "untyped",
        ),
        (
            r###"["ok"].reduce(JSON.parse(raw), JSON.parse('"+"'))"###,
            true,
            "untyped",
        ),
        (
            r###"[JSON.parse(raw)].reduce(JSON.parse('"+"'))"###,
            true,
            "untyped",
        ),
        (
            r###"[JSON.parse(raw)].each_with_object("ok") { |x, seed| x }"###,
            false,
            "untyped",
        ),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn nested_serialization_never_inherits_keys_or_values() {
    for (body, gradual, forward) in [
        (r###"[{JSON.parse('"k"') => "ok"}].as_json.first["k"]"###, false, "untyped"),
        (r###"{"outer" => {JSON.parse('"k"') => "ok"}}.as_json["outer"]["k"]"###, false, "untyped"),
        (r###"[[{JSON.parse('"k"') => "ok"}]].as_json.first.first["k"]"###, false, "untyped"),
        (r###"{"outer" => [{JSON.parse('"k"') => "ok"}]}.as_json["outer"].first["k"]"###, false, "untyped"),
        (r###"[{JSON.parse('["k"]') => "ok"}].as_json.first.values.first"###, false, "untyped"),
        (r###"[{JSON.parse('{"k":1}') => "ok"}].as_json.first.values.first"###, false, "untyped"),
        (r###"[{JSON.parse('"k"') => "ok"}].send(:as_json).first["k"]"###, false, "untyped"),
        (r###"{"outer" => {JSON.parse('"k"') => "ok"}}.public_send("as_json")["outer"]["k"]"###, false, "untyped"),
        (r###"items = [{JSON.parse('"k"') => "ok"}]; options = JSON.parse("{}"); items.as_json(options) { JSON.parse(raw) }.first["k"]"###, false, "untyped"),
        (r###"[{"k" => JSON.parse('"ok"')}].as_json.first["k"]"###, false, "untyped"),
        (r###"{"outer" => {"k" => JSON.parse('"ok"')}}.as_json["outer"]["k"]"###, false, "untyped"),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn hash_constructors_keep_main_for_every_operand_form() {
    for (body, gradual, forward) in [
        (r###"Hash[JSON.parse('"k"'), "ok"]["k"]"###, false, "untyped"),
        (r###"Hash[[[JSON.parse('"k"'), "ok"]]]["k"]"###, false, "untyped"),
        (r###"Hash[{JSON.parse('"k"') => "ok"}]["k"]"###, false, "untyped"),
        (r###"Hash[JSON.parse('"k"'), "ok", "other", "ok"]["k"]"###, false, "untyped"),
        (r###"Hash["other", "ok", JSON.parse('"k"'), "ok"]["k"]"###, false, "untyped"),
        (r###"Hash[[[JSON.parse('"k"'), "ok"], ["other", "ok"]]]["k"]"###, false, "untyped"),
        (r###"Hash[[["other", "ok"], [JSON.parse('"k"'), "ok"]]]["k"]"###, false, "untyped"),
        (r###"Hash["k", JSON.parse('"ok"')]["k"]"###, false, "untyped"),
        (r###"Hash[[["k", JSON.parse('"ok"')]]]["k"]"###, false, "untyped"),
        (r###"Hash[{"k" => JSON.parse('"ok"')}]["k"]"###, false, "untyped"),
        (r###"Hash.send(:[], JSON.parse('"k"'), "ok")["k"]"###, false, "untyped"),
        (r###"Hash.public_send("[]", [[JSON.parse('"k"'), "ok"]])["k"]"###, false, "untyped"),
        (r###"Hash.__send__(:send, :[], {JSON.parse('"k"') => "ok"})["k"]"###, false, "untyped"),
        (r###"key = JSON.parse('"k"'); pairs = [[key, "ok"]]; Hash[pairs]["k"]"###, false, "untyped"),
        (r###"Hash[JSON.parse('"k"'), "ok"] { JSON.parse(raw) }["k"]"###, false, "untyped"),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}

#[test]
fn exec_binding_and_destructuring_keep_main() {
    for (body, gradual, forward) in [
        (r###"Object.new.instance_exec(["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
        (r###"Probe.class_exec(["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
        (r###"Probe.module_exec(["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
        (r###"Object.new.instance_exec([JSON.parse(raw), "ok"]) { |other, value| value }"###, false, "untyped"),
        (r###"Object.new.instance_exec("ok", JSON.parse(raw)) { |value, other| value }"###, false, "untyped"),
        (r###"Object.new.instance_exec(JSON.parse(raw), "ok") { |other, value| value }"###, false, "untyped"),
        (r###"Object.new.instance_exec(["ok", JSON.parse(raw)]) { |value, *others| value }"###, false, "untyped"),
        (r###"Object.new.instance_exec(["ok", JSON.parse(raw)]) { |(value, other)| value }"###, false, "untyped"),
        (r###"args = ["ok", JSON.parse(raw)]; Object.new.instance_exec(args) { |value, other| value }"###, false, "untyped"),
        (r###"Object.new.send(:instance_exec, ["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
        (r###"Probe.public_send(:class_exec, ["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
        (r###"Probe.__send__(:send, :module_exec, ["ok", JSON.parse(raw)]) { |value, other| value }"###, false, "untyped"),
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, gradual, forward);
            }
        }
    }
}
