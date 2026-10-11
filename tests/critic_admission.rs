//! Regression cases where inference used to certify an unexecutable program.
//! Both CLI gates must refuse; passing `check` alone is not admission evidence.
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn refuses(body: &str, expected: &str) {
    refuses_with(body, expected, &[]);
}

fn refuses_with(body: &str, expected: &str, extra: &[(&str, &str)]) {
    let root = std::env::temp_dir().join(format!("rh_admission_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    for (path, text) in [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n".to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n".to_string()),
        ("app/models/probe.rb", format!("class Probe < ApplicationRecord\n  validates :title, presence: true\n  def probe\n    {body}\n  end\nend\n")),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    for (path, text) in extra {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    for args in [vec!["check"], vec!["--target", "ruby"]] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
        command.args(&args).arg(&root);
        if args[0] != "check" { command.arg("-o").arg(root.join("emitted")); }
        let output = command.output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!output.status.success(), "{args:?} accepted {body}: {text}");
        assert!(text.contains(expected), "{args:?} lost named refusal {expected}: {text}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn module_protocol_requires_a_module_object() {
    for body in ["self.define_method(:x) { 1 }", "define_method(:x) { 1 }", "class_eval { 2 }", "\"abc\".define_method(:x) { 1 }", "1.class_eval { 2 }", "self.const_get(:X)", "self.include?(1)"] {
        refuses(body, "Module protocol");
    }
}

#[test]
fn unresolved_declared_classes_are_errors() {
    refuses("v = T.let(1, GhostMoney); v.imaginary_method", "GhostMoney");
    refuses("v = T.let(1, GhostMoney); v.message.upcase", "GhostMoney");
}

#[test]
fn new_errors_api_requires_runtime_support() {
    for body in ["errors.added?(:title, :blank)", "errors.details", "errors.where(:title)", "errors.merge!(errors)", "errors.first.options", "errors.delete(:title); errors.size"] {
        refuses(body, "ActiveModel::Errors");
    }
}

#[test]
fn non_nil_assertions_require_runtime_support() {
    // `T.must` lowers to sorbet's check (tests/sorbet_must_runs.rs).
    refuses("T.must_because(1) { \"present\" }", "non-nil assertion");
    refuses("1.not_nil!", "not_nil!");
}

#[test]
fn for_scope_is_not_a_block_scope() {
    for body in [
        "x = 10; for x in [1, 2]; end; x",
        "for x in []; end; [defined?(x), x]",
        "for x in [1, 2]; y = x * 10; end; y",
        "for p, q in [[1, 2], [3, 4]]; end; [p, q]",
        "for x in []; y = 1; end; [defined?(y), y]",
    ] { refuses(body, "for"); }
}

#[test]
fn registry_signatures_do_not_supply_missing_runtime() {
    for (body, name) in [
        ("Rails.event.notify(\"probe\")", "event"),
        ("Rails.error.set_context(probe: true)", "error"),
        ("ActiveRecord::Base.connected_to(role: :reading) { 7 }", "connected_to"),
        ("Benchmark.realtime { 1 }", "Benchmark"),
        ("\"2020-01-02\".to_date", "to_date"),
        ("\"1.5\".to_d", "to_d"),
        ("{}.to_sentence", "to_sentence"),
    ] { refuses(body, name); }
}

#[test]
fn application_tracers_are_not_global_stdlib() {
    refuses("ShopifyTracer.in_span(\"probe\") { 42 }", "ShopifyTracer");
}

#[test]
fn a_nominal_class_does_not_prove_operator_ownership() {
    refuses_with("Value.new + 1", "incompatible_binop", &[("app/lib/value.rb", "class Value\nend\n")]);
}

#[test]
fn unresolved_includes_are_load_time_errors() {
    refuses_with("Imported.new.zork", "GhostModule", &[("app/lib/imported.rb", "class Imported\n  include GhostModule\n  def zork; 1; end\nend\n")]);
}

#[test]
fn identity_is_invalidated_by_reassignment_and_branch_writes() {
    for body in [
        "k = Probe; k = Probe.new; k.define_method(:x) { 1 }",
        "k = Probe; if title; k = Probe.new; end; k.define_method(:x) { 1 }",
        "k = Probe; [1].each { |k| k.define_method(:x) { 1 } }",
        "k = Probe; [1].each { |*k| k.define_method(:x) { 1 } }",
        "k = Probe; k, x = [Probe.new, 1]; k.define_method(:x) { 1 }",
        "k = Probe; [Probe.new] => [k]; k.define_method(:x) { 1 }",
        "k = Probe; begin; raise StandardError; rescue => k; k.define_method(:x) { 1 }; end",
        "Probe.new.class.define_method(:x) { 1 }",
    ] { refuses(body, "Module protocol"); }
}

#[test]
fn cross_date_time_comparison_requires_activesupport_runtime() {
    refuses("Date.new(2020, 1, 1) < Time.utc(2020, 1, 2)", "incompatible_binop");
}

#[test]
fn instance_eval_changes_self_identity_in_class_side_methods() {
    refuses_with("Probe.go", "Module protocol", &[("app/models/probe.rb", "class Probe < ApplicationRecord\n  def self.go\n    Probe.new.instance_eval { self.define_method(:x) { 1 } }\n  end\nend\n")]);
}

#[test]
fn unused_unresolved_model_include_is_not_silent() {
    refuses_with("1", "GhostModule", &[("app/models/probe.rb", "class Probe < ApplicationRecord\n  include GhostModule\nend\n")]);
}

#[test]
fn object_extensions_without_runtime_are_refused() {
    // `to_param` on an AR model is synthesized + registered; refuse it on
    // a plain library object that has no runtime/lowering support.
    for body in ["self.to_query", "self.instance_values", "self.acts_like?(:probe)", "self.presence_in([1])"] {
        refuses(body, "Object extension");
    }
    refuses_with(
        "Value.new.to_param",
        "Object extension",
        &[("app/lib/value.rb", "class Value\nend\n")],
    );
}

#[test]
fn lambda_parameters_cannot_borrow_outer_class_identity() {
    refuses("k = Probe; f = ->(k) { k.define_method(:x) { 1 } }; f.call(Probe.new)", "Module protocol");
}

#[test]
fn dropped_block_capture_cannot_borrow_outer_identity() {
    refuses("k = Probe; [1].each { |&k| k.define_method(:x) { 1 } }", "block parameter capture");
    refuses("k = Probe; f = ->(&k) { k.define_method(:x) { 1 } }; f.call", "block parameter capture");
}

#[test]
fn framework_prefix_does_not_prove_an_include_exists() {
    refuses_with("1", "ActiveModel::Ghost", &[("app/models/probe.rb", "class Probe < ApplicationRecord\n  include ActiveModel::Ghost\nend\n")]);
}

#[test]
fn native_module_callback_contract_is_not_an_instance_escape() {
    refuses_with("Hook.included(Probe.new)", "Module callback argument", &[("app/helpers/hook.rb", "module Hook\n  def self.included(base)\n    base.define_method(:hook_value) { 23 }\n  end\nend\n")]);
}

#[test]
fn library_nominal_operator_failures_are_not_hidden() {
    refuses_with("1", "incompatible_binop", &[("app/helpers/value.rb", "class Value\n  def bad\n    Value.new + 1\n  end\nend\n")]);
    for value_definition in ["class Value\nend\n", "class Value\n  def +(other); other; end\nend\n"] {
        let library = format!("{value_definition}class Other\nend\nclass UnionOperator\n  def bad(flag)\n    value = flag ? Value.new : Other.new\n    value + 1\n  end\nend\n");
        refuses_with("1", "incompatible_binop", &[("app/helpers/value.rb", &library)]);
    }
}
