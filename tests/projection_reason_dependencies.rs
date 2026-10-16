//! Computed projections retain main behavior even with gradual operands.

use std::{collections::HashMap, path::PathBuf};

use roundhouse::analyze::{DiagnosticKind, diagnose};

fn check(body: &str, instance: bool, depth: usize, gradual: bool) {
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
    let mut tree = HashMap::from([
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
    if body.contains("Item.") {
        tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n create_table :items do |t|\n t.string :name\n end\nend\n".to_vec());
        tree.insert(
            PathBuf::from("app/models/item.rb"),
            b"class Item < ActiveRecord::Base\nend\n".to_vec(),
        );
    }
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let rbs = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let mut expected = vec![format!(
        "def {prefix}value: (String raw) -> {}",
        if gradual { "Symbol" } else { "String" }
    )];
    expected.extend((0..depth).map(|i| format!("def {prefix}forward{i}: (String raw) -> untyped")));
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

    {
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
fn unrelated_gradual_elements_do_not_poison_concrete_results() {
    for body in [
        "[JSON.parse(raw)].reduce(\"ok\") { |acc, x| \"ok\" }",
        "[JSON.parse(raw)].inject(\"ok\") { |acc, x| acc }",
        "[JSON.parse(raw)].each_with_object(\"ok\") { |x, acc| x }",
        "[JSON.parse(raw)].each.with_object(\"ok\") { |x, acc| x }",
        "[JSON.parse(raw)].group_by { |x| \"ok\" }.keys.first",
        "[JSON.parse(raw)].index_by { |x| \"ok\" }.keys.first",
        "[JSON.parse(raw)].sum(\"\") { |x| \"ok\" }",
        "{key: JSON.parse(raw)}.reduce(\"ok\") { |acc, pair| \"ok\" }",
        "{key: JSON.parse(raw)}.inject(\"ok\") { |acc, pair| acc }",
        "{key: JSON.parse(raw)}.each_with_object(\"ok\") { |pair, acc| pair }",
        "{key: JSON.parse(raw)}.sum(\"\") { |pair| \"ok\" }",
        "[[JSON.parse(raw)], 1].reduce(\"ok\") { |acc, x| \"ok\" }",
        "[1, {key: JSON.parse(raw)}].group_by { |x| \"ok\" }.keys.first",
        "seed = \"ok\"; items = [JSON.parse(raw)]; items.reduce(seed) { |acc, x| acc }",
        "items = [JSON.parse(raw)]; items.instance_variable_set(:@x, \"ok\")",
        "items = [JSON.parse(raw)]; items.send(:instance_variable_set, :@x, \"ok\")",
        "[JSON.parse(raw)]&.reduce(\"ok\") { |acc, x| \"ok\" }",
        "[JSON.parse(raw)].send(:reduce, \"ok\") { |acc, x| acc }",
        "[JSON.parse(raw)].public_send(\"inject\", \"ok\") { |acc, x| acc }",
        "[JSON.parse(raw)].__send__(:send, :reduce, \"ok\") { |acc, x| acc }",
        "[[JSON.parse(raw)], nil].first&.reduce(\"ok\") { |acc, x| acc }",
        "[[JSON.parse(raw)], {key: 1}].first.reduce(\"ok\") { |acc, x| acc }",
        "[{key: 1}, [JSON.parse(raw)]].first.inject(\"ok\") { |acc, x| acc }",
        "[JSON.parse(raw)].send(:group_by) { |x| \"ok\" }.keys.first",
        "[JSON.parse(raw)].public_send(\"index_by\") { |x| \"ok\" }.keys.first",
        "{key: JSON.parse(raw)}.with_indifferent_access.reduce(\"ok\") { |acc, pair| acc }",
        "{key: JSON.parse(raw)}.with_indifferent_access.each_with_object(\"ok\") { |pair, acc| pair }",
        "Item.all.reduce(\"ok\") { |acc, item| \"ok\" }",
        "Item.all.group_by { |item| \"ok\" }.keys.first",
        "Item.all.each_with_object(\"ok\") { |item, acc| JSON.parse(raw) }",
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, false);
            }
        }
    }
}

#[test]
fn computed_seeds_and_block_results_keep_main_forwarding_declarations() {
    for body in [
        "[1].reduce(JSON.parse(raw)) { |acc, x| acc }",
        "[1].inject(JSON.parse(raw)) { |acc, x| acc }",
        "[1].reduce(JSON.parse(raw)) { |acc, x| \"ok\" }",
        "[1].each_with_object(JSON.parse(raw)) { |x, acc| \"ok\" }",
        "[1].each.with_object(JSON.parse(raw)) { |x, acc| \"ok\" }",
        "[1].sum(JSON.parse(raw))",
        "[1].sum(JSON.parse(raw)) { |x| 1 }",
        "[1].reduce(\"ok\") { |acc, x| JSON.parse(raw) }",
        "[1].group_by { |x| JSON.parse(raw) }.keys.first",
        "[1].index_by { |x| JSON.parse(raw) }.keys.first",
        "[JSON.parse(raw)].group_by { |x| \"ok\" }.values.first.first",
        "[JSON.parse(raw)].index_by { |x| \"ok\" }.values.first",
        "[JSON.parse(raw)].reduce { |acc, x| \"ok\" }",
        "[JSON.parse(raw)].inject(:+)",
        "[1].reduce(JSON.parse(raw), :+)",
        "[1].inject(JSON.parse(raw), \"+\")",
        "[].reduce(JSON.parse(raw)) { |acc, x| \"ok\" }",
        "[].each_with_object(JSON.parse(raw)) { |x, acc| \"ok\" }",
        "{key: 1}.reduce(JSON.parse(raw)) { |acc, pair| acc }",
        "{key: 1}.inject(JSON.parse(raw)) { |acc, pair| acc }",
        "{key: 1}.each_with_object(JSON.parse(raw)) { |pair, acc| pair }",
        "{key: 1}.sum(JSON.parse(raw)) { |pair| 1 }",
        "[1].reduce([JSON.parse(raw)]) { |acc, x| acc }",
        "[1].each_with_object({key: JSON.parse(raw)}) { |x, acc| x }",
        "seed = JSON.parse(raw); items = [1]; items.inject(seed) { |acc, x| acc }",
        "[1]&.reduce(JSON.parse(raw)) { |acc, x| acc }",
        "[1].send(:reduce, JSON.parse(raw)) { |acc, x| acc }",
        "[1].public_send(\"inject\", JSON.parse(raw)) { |acc, x| acc }",
        "[1].__send__(:send, :each_with_object, JSON.parse(raw)) { |x, acc| x }",
        "(1..2).reduce(JSON.parse(raw)) { |acc, x| acc }",
        "(1..2).each_with_object(JSON.parse(raw)) { |x, acc| x }",
        "seed = raw.empty? ? JSON.parse(raw) : 1; [1].reduce(seed) { |acc, x| acc }",
        "seed = raw.empty? ? 1 : JSON.parse(raw); [1].reduce(seed) { |acc, x| acc }",
        "[[1], nil].first&.reduce(JSON.parse(raw)) { |acc, x| acc }",
        "[[1], {key: 1}].first.reduce(JSON.parse(raw)) { |acc, x| acc }",
        "[{key: 1}, [1]].first.inject(JSON.parse(raw)) { |acc, x| acc }",
        "{key: 1}.with_indifferent_access.inject(JSON.parse(raw)) { |acc, pair| acc }",
        "{key: 1}.with_indifferent_access.each_with_object(JSON.parse(raw)) { |pair, acc| pair }",
        "Item.all.reduce(JSON.parse(raw)) { |acc, item| acc }",
        "Item.all.inject(JSON.parse(raw)) { |acc, item| acc }",
        "Item.all.each_with_object(JSON.parse(raw)) { |item, acc| item }",
        "Item.all.group_by { |item| JSON.parse(raw) }.keys.first",
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, true);
            }
        }
    }
}

#[test]
fn reflective_computations_keep_main_forwarding_declarations() {
    for method in ["instance_variable_set", "class_variable_set"] {
        let name = if method == "class_variable_set" {
            "@@x"
        } else {
            "@x"
        };
        for body in [
            format!("self.{method}(:{name}, JSON.parse(raw))"),
            format!("self.{method}(\"{name}\", [JSON.parse(raw)])"),
            format!("value = JSON.parse(raw); self.{method}(:{name}, value)"),
            format!("self.send(:{method}, :{name}, JSON.parse(raw))"),
            format!("self.public_send(\"{method}\", :{name}, JSON.parse(raw))"),
            format!("self.__send__(:send, :{method}, :{name}, JSON.parse(raw))"),
            format!("{method}(:{name}, JSON.parse(raw))"),
            format!("self.{method}(:{name}, {{key: JSON.parse(raw)}})"),
        ] {
            // class_variable_set requires a class/module object.
            for instance in if method == "class_variable_set" {
                vec![false]
            } else {
                vec![false, true]
            } {
                for depth in [2, 4] {
                    check(&body, instance, depth, true);
                }
            }
        }
        for body in [
            format!("self.{method}(:{name}, \"ok\")"),
            format!("self.send(:{method}, :{name}, \"ok\")"),
            format!("self.public_send(\"{method}\", :{name}, \"ok\")"),
            format!("name = JSON.parse('\"{name}\"'); self.{method}(name, \"ok\")"),
            format!("self.{method}(:{name}, \"ok\") {{ JSON.parse(raw) }}"),
        ] {
            check(&body, false, 2, false);
        }
    }
    for body in [
        "Object.new.instance_variable_set(:@x, JSON.parse(raw))",
        "\"s\".instance_variable_set(:@x, JSON.parse(raw))",
        "{}.instance_variable_set(:@x, JSON.parse(raw))",
        "[].instance_variable_set(:@x, JSON.parse(raw))",
    ] {
        check(body, false, 2, true);
    }
}

#[test]
fn catalog_callback_fallbacks_are_computations_not_gradual_sources() {
    for body in [
        "Item.new.with_lock { \"ok\" }",
        "Item.new.with_lock(JSON.parse('\"FOR UPDATE\"')) { \"ok\" }",
        "Item.new.with_lock(requires_new: JSON.parse(\"true\")) { \"ok\" }",
        "Item.new.send(:with_lock) { \"ok\" }",
        "Item.new.public_send(\"with_lock\") { \"ok\" }",
        "Item.new.__send__(:send, :with_lock) { \"ok\" }",
        "item = Item.new; item.with_lock { \"ok\" }",
    ] {
        for instance in [false, true] {
            for depth in [2, 4] {
                check(body, instance, depth, false);
            }
        }
    }
}
