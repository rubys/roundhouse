//! Integration boundaries that must not turn source forwarding into a clean
//! but changed program: class fragments, relation ABI, nested reconstruction.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

#[test]
fn reopened_class_contracts_refuse_both_orders_and_late_includes() {
    let full =
        "class Probe; def self.call(...); target(...); end; def self.target(...); 695; end; end";
    let flat = "class Probe; def self.target(a,b,factor:2); (a-b)*factor; end; end";
    for (first, last, expected) in [
        (full, flat, "21\n"),
        (flat, full, "695\n"),
        (
            "module Full; def target(...); 695; end; end; class Probe; include Full; def call(...); target(...); end; end",
            "module Flat; def target(a,b,factor:2); (a-b)*factor; end; end; class Probe; include Flat; end",
            "21\n",
        ),
    ] {
        let script = if first.contains("self.call") || last.contains("self.call") {
            "puts Probe.call(11,4,factor:3)"
        } else {
            "puts Probe.new.call(11,4,factor:3)"
        };
        let native = Command::new("ruby")
            .args(["-e", &format!("{first}; {last}; {script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", first)
            .write("app/lib/probe_reopen.rb", last)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding")),
            "{:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn full_declaration_shadowing_an_ingest_builtin_is_not_admitted() {
    let native = Command::new("ruby")
        .args(["-e", "class Article; def self.exists?(...); 11; end; end; class Probe; def self.run; Article.exists?(id:1); end; end; puts Probe.run"])
        .output().unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def self.exists?(...); 11; end\n",
        )
        .write(
            "app/lib/probe.rb",
            "class Probe; def self.run; Article.exists?(id:1); end; end",
        )
        .run_ruby("puts Probe.run");
    // Ingest rewrites this selector even for an app-defined declaration.
    // The declaration must be refused before the erased call can look clean.
    assert!(
        run.errors.iter().any(|e| e.contains("ingest-time call rewrite")),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn native_full_scope_declaration_refuses_relation_threading() {
    let source = "class ScopeProbe; def self.order(key); key; end; def self.recent(...); order(:id); end; end; puts ScopeProbe.recent(11,4,factor:3)";
    let native = Command::new("ruby").args(["-e", source]).output().unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "id\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def self.recent(...); order(:id); end\n",
        )
        .run_ruby("puts 'loaded'");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("relation") && e.contains("forwarding")),
        "{:?}; stderr={}",
        run.errors,
        run.stderr
    );
    let model = std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap();
    assert!(!model.contains("def self.recent(..., __rel"), "{model}");
}

#[test]
fn association_demand_refuses_full_declarations_even_without_a_query_body() {
    // This native control proves the source class declaration's value, not
    // Rails relation delegation. The overlay exercises that separate ABI seam.
    let native = Command::new("ruby")
        .args([
            "-e",
            "class Comment; def self.recent(...); 11; end; end; puts Comment.recent(11,4,factor:3)",
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            "class Comment < ApplicationRecord\n def self.recent(...); 11; end\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def forwarded_comments; comments.recent(11,4,factor:3); end\n",
        )
        .run_ruby("puts 'loaded'");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("relation-threading argument ABI")),
        "{:?}; stderr={}",
        run.errors,
        run.stderr
    );
    let model = std::fs::read_to_string(run.emitted.join("app/models/comment.rb")).unwrap();
    assert!(!model.contains("def self.recent(..., __rel"), "{model}");
}

#[test]
fn native_keyword_value_is_rewritten_inside_emitted_controller() {
    let helper = "class Forwarder; def self.accept(...); 11; end; end";
    let control = format!(
        "{helper}; class Params; def expect(key); 23; end; end; puts Forwarder.accept(**{{id: Params.new.expect(:id)}})"
    );
    let native = Command::new("ruby")
        .args(["-e", &control])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/forwarder.rb", helper)
        .edit(
            "app/controllers/articles_controller.rb",
            "  def show\n  end",
            "  def show\n    Forwarder.accept(**{id: params.expect(:id)})\n  end",
        )
        .run_test("test/controllers/articles_controller_test.rb");
    run.assert_passes();
    let controller =
        std::fs::read_to_string(run.emitted.join("app/controllers/articles_controller.rb"))
            .unwrap();
    assert!(controller.contains("Forwarder.accept(**"), "{controller}");
    assert!(!controller.contains("params.expect"), "{controller}");
}

#[test]
fn inherited_constructor_and_reopened_descendant_mixin_refuse_unsafe_contracts() {
    // Native keyword declarations now execute the inherited constructor.
    // A reopened class fragment still cannot prove the forwarding contract.
    let executing = [(
        "class Parent; def self.build(...); new(...); end; def initialize(factor:); @factor=factor; end; def factor; @factor; end; end; class Child < Parent; def initialize(factor:2); @factor=factor; end; end",
        "",
        "puts Child.build(factor:3).factor",
    )];
    let refused = [(
        "module Mix; def target(factor:); factor; end; end; class Parent; def call(...); target(...); end; def target(factor:); factor*2; end; end; class Child < Parent; include Mix; end",
        "class Parent; def target(factor:2); factor; end; end",
        "puts Child.new.call(factor:3)",
    )];
    for (source, reopen, script) in executing {
        let native = Command::new("ruby")
            .args(["-e", &format!("{source};{reopen};{script}")])
            .output()
            .unwrap();
        assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
        assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", source)
            .write("app/lib/probe_reopen.rb", reopen)
            .run_ruby(script);
        run.assert_passes();
        assert_eq!(run.stdout, "3\n");
    }
    for (source, reopen, script) in refused {
        let native = Command::new("ruby")
            .args(["-e", &format!("{source};{reopen};{script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", source)
            .write("app/lib/probe_reopen.rb", reopen)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding") && e.contains("reopened")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn framework_wrappers_refuse_full_packets_instead_of_empty_name_arguments() {
    let sink = "class Sink; def self.target(a,b,factor:); yield((a-b)*factor); end; end";
    for (source, stub, path, script) in [
        (
            "class ProbeJob < ActiveJob::Base; def perform(...); Sink.target(...); end; end",
            "module ActiveJob; class Base; def self.perform_now(...); new.perform(...); end; end; end",
            "app/jobs/probe_job.rb",
            "puts ProbeJob.perform_now(11,4,factor:3) { |r| r*2+1 }",
        ),
        (
            "class ProbeMailer < ActionMailer::Base; def notify(...); Sink.target(...); end; end",
            "module ActionMailer; class Base; def self.notify(...); new.notify(...); end; end; end",
            "app/mailers/probe_mailer.rb",
            "puts ProbeMailer.notify(11,4,factor:3) { |r| r*2+1 }",
        ),
    ] {
        // Isolate the native forwarding behavior of the wrapper. This is not
        // a claim to emulate Rails delivery/queuing or Proc lifting.
        let native = Command::new("ruby")
            .args(["-e", &format!("{stub};{sink};{source};{script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), "43\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/sink.rb", sink)
            .write(path, source)
            .run_ruby(script);
        assert!(
            run.errors
                .iter()
                .any(|e| e.contains("wrapper") && e.contains("full forwarding")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn runtime_attribute_reconstruction_retains_source_formal_facts() {
    use roundhouse::dialect::UnsupportedFormal;
    for formal in ["**nil", "&"] {
        let ruby = format!(
            "class Probe; def value({formal}); raise NotImplementedError; end; def value=(x); raise NotImplementedError; end; end"
        );
        let classes = roundhouse::runtime_src::parse_library_with_rbs(
            ruby.as_bytes(),
            "class Probe\n def value: () -> Integer\n def value=: (Integer x) -> Integer\nend",
            "probe.rb",
        )
        .unwrap();
        let method = classes[0]
            .methods
            .iter()
            .find(|m| m.name.as_str() == "value")
            .unwrap();
        if formal == "**nil" {
            assert_eq!(
                method.unsupported_formals,
                Some(UnsupportedFormal::NoKeywords)
            );
        } else {
            assert!(method.has_anonymous_block);
        }
    }
}

#[test]
fn copied_constructor_result_and_local_cannot_prove_the_base_contract() {
    for body in ["new.target(...)", "receiver=new; receiver.target(...)"] {
        let source = format!(
            "class Parent; def self.build(...); {body}; end; def target(factor:); factor; end; end; class Child < Parent; def target(factor:2); factor; end; end"
        );
        let script = "puts Child.build(factor:3)";
        let native = Command::new("ruby")
            .args(["-e", &format!("{source};{script}")])
            .output()
            .unwrap();
        assert!(native.status.success());
        assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", &source)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn synthesized_model_method_does_not_silently_replace_a_full_source_contract() {
    let method = "def self._conflict_predicate(...); 11; end";
    let probe =
        "class Probe; def self.run; kw={factor:3}; Article._conflict_predicate(7,**kw); end; end";
    let native = Command::new("ruby")
        .args([
            "-e",
            &format!("class Article; {method}; end; {probe}; puts Probe.run"),
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    for partial_index in [false, true] {
        let mut overlay = emit_and_run::real_blog()
            .edit(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                &format!("class Article < ApplicationRecord\n {method}\n"),
            )
            .write("app/lib/probe.rb", probe);
        if partial_index {
            overlay = overlay.edit(
                "db/schema.rb",
                "    t.string \"title\"",
                "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
            );
        }
        let run = overlay.run_ruby("puts Probe.run");
        if partial_index {
            assert!(
                run.errors
                    .iter()
                    .any(|e| e.contains("model method synthesis")),
                "errors={:?}; actual={}; stderr={}",
                run.errors,
                run.stdout,
                run.stderr
            );
        } else {
            run.assert_passes();
            assert_eq!(run.stdout, "11\n");
        }
    }
}

#[test]
fn inherited_full_model_contract_cannot_be_replaced_by_subclass_synthesis() {
    for (name, partial_index, refused) in [
        ("_conflict_predicate", false, false),
        ("_conflict_predicate", true, true),
        // The generated instance reader must not shadow a class contract.
        ("title", true, false),
    ] {
        let method = format!("def self.{name}(...); 11; end");
        let probe =
            format!("class Probe; def self.run; kw={{factor:3}}; Article.{name}(7,**kw); end; end");
        let native = Command::new("ruby").args(["-e", &format!(
            "class ForwardingRecord; {method}; end; class Article < ForwardingRecord; end; {probe}; puts Probe.run"
        )]).output().unwrap();
        assert!(native.status.success());
        assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
        let mut overlay = emit_and_run::real_blog()
            .write("app/models/forwarding_record.rb", &format!("class ForwardingRecord < ApplicationRecord; self.abstract_class = true; {method}; end"))
            .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ForwardingRecord")
            .write("app/lib/probe.rb", &probe);
        if partial_index {
            overlay = overlay.edit(
                "db/schema.rb",
                "    t.string \"title\"",
                "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
            );
        }
        let run = overlay.run_ruby("puts Probe.run");
        if refused {
            assert!(
                run.errors
                    .iter()
                    .any(|e| e.contains("model method synthesis")),
                "{:?}; {}",
                run.errors,
                run.stderr
            );
        } else {
            run.assert_passes();
            assert_eq!(run.stdout, "11\n");
        }
    }
}

#[test]
fn keyword_packet_keeps_unaffected_base_and_sibling_contracts() {
    let parent = r#"
class ForwardingRecord < ApplicationRecord
  self.abstract_class = true
  def self._conflict_predicate(**); :inherited; end
end
"#;
    let descendants = r#"
class ChildArticle < Article; end
class SiblingRecord < ForwardingRecord; end
"#;
    let probe = r#"
class Probe
  def self.base(**)
    ForwardingRecord._conflict_predicate(**)
  end

  def self.sibling(**)
    SiblingRecord._conflict_predicate(**)
  end
end
"#;

    let run = emit_and_run::real_blog()
        .write("app/models/forwarding_record.rb", parent)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ForwardingRecord",
        )
        .write("app/lib/keyword_forwarding_descendants.rb", descendants)
        .write("app/lib/probe.rb", probe)
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
        )
        .run_ruby(
            "raise 'base dispatch changed' unless Probe.base(value: 11) == :inherited; raise 'sibling dispatch changed' unless Probe.sibling(value: 12) == :inherited; puts 'base and sibling keyword calls still dispatch'",
        );
    run.assert_passes();
    assert!(run.errors.is_empty(), "errors={:?}", run.errors);
    assert_eq!(run.stdout, "base and sibling keyword calls still dispatch\n");
}

#[test]
fn keyword_super_packet_to_plain_parent_emits_and_executes() {
    let classes = r#"
class PacketParent
  def self.route(**)
    :parent_contract
  end
end

class PacketChild < PacketParent
  def self.route(**)
    super(**)
  end
end
"#;
    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_super_packets.rb", classes)
        .run_ruby(
            "raise 'super keyword packet changed' unless PacketChild.route(value: 17) == :parent_contract; puts 'plain parent keyword contract preserved'",
        );

    run.assert_passes();
    assert!(run.errors.is_empty(), "errors={:?}", run.errors);
    assert_eq!(run.stdout, "plain parent keyword contract preserved\n");
}

#[test]
fn keyword_packet_through_super_checks_replaced_model_ancestor() {
    let parent = r#"
class ForwardingRecord < ApplicationRecord
  self.abstract_class = true
  def self._conflict_predicate(**); :source; end
end
"#;
    let child = r#"
class ChildArticle < Article
  def self._conflict_predicate(**)
    super(**)
  end
end
"#;
    // The source keyword-rest contract accepts this call. Article's partial
    // index instead synthesizes a positional columns method; `super(**)`
    // reaches that replacement from the child's inherited lookup chain.
    let native_source = r#"
class ForwardingRecord
  def self._conflict_predicate(**); :source; end
end
class Article < ForwardingRecord
  def self._conflict_predicate(columns); columns.join(","); end
end
class ChildArticle < Article
  def self._conflict_predicate(**); super(**); end
end
begin
  ChildArticle._conflict_predicate(columns: [:id])
  raise "super packet unexpectedly matched the source keyword-rest contract"
rescue NoMethodError => error
  raise unless error.name == :join
end
puts "super packet reaches the synthesized positional contract"
"#;
    let native = Command::new("ruby")
        .args(["-e", native_source])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "super packet reaches the synthesized positional contract\n"
    );

    let run = emit_and_run::real_blog()
        .write("app/models/forwarding_record.rb", parent)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ForwardingRecord",
        )
        .write("app/lib/keyword_forwarding_child.rb", child)
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
        )
        .run_ruby("puts 'super packet into a replaced model ancestor is diagnosed'");
    assert!(run.success, "actual={}; stderr={}", run.stdout, run.stderr);
    assert_eq!(
        run.stdout,
        "super packet into a replaced model ancestor is diagnosed\n"
    );
    assert_eq!(run.errors.len(), 1, "errors={:?}", run.errors);
    assert!(
        run.errors[0].contains("model method synthesis"),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn keyword_packet_checks_inherited_model_contract_replaced_by_synthesis() {
    let parent = r#"
class ForwardingRecord < ApplicationRecord
  self.abstract_class = true
  def self._conflict_predicate(**); :inherited; end
  def self.virtual_relay(**); _conflict_predicate(**); end
end
"#;
    let descendants = r#"
class ChildArticle < Article; end
class SiblingRecord < ForwardingRecord; end
"#;
    let probe = r#"
class Probe
  def self.relay(**)
    Article._conflict_predicate(**)
  end

  def self.mixed(**)
    Article._conflict_predicate(kind: :generic, **)
  end

  def self.child(**)
    ChildArticle._conflict_predicate(**)
  end
end
"#;
    let native_source = r##"
class ForwardingRecord
  def self._conflict_predicate(**); :inherited; end
  def self.virtual_relay(**); _conflict_predicate(**); end
end
class Article < ForwardingRecord
  def self._conflict_predicate(columns); :generated; end
end
class ChildArticle < Article; end
class SiblingRecord < ForwardingRecord; end
class Probe
  def self.base(**); ForwardingRecord._conflict_predicate(**); end
  def self.sibling(**); SiblingRecord._conflict_predicate(**); end
  def self.relay(**); Article._conflict_predicate(**); end
  def self.mixed(**); Article._conflict_predicate(kind: :generic, **); end
  def self.child(**); ChildArticle._conflict_predicate(**); end
end
raise "base dispatch changed" unless Probe.base(value: 11) == :inherited
raise "sibling dispatch changed" unless Probe.sibling(value: 12) == :inherited
[[Article, :single], [Article, :mixed], [ChildArticle, :single], [Article, :virtual]].each do |klass, mode|
  actual = case mode
  when :virtual then klass.virtual_relay(value: 13)
  when :mixed then klass._conflict_predicate(kind: :generic, value: 13)
  else klass._conflict_predicate(value: 13)
  end
  raise "#{klass} #{mode} did not select the synthesized method" unless actual == :generated
end
puts "base and sibling keyword calls still dispatch"
"##;
    let native = Command::new("ruby")
        .args(["-e", native_source])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "base and sibling keyword calls still dispatch\n"
    );

    let run = emit_and_run::real_blog()
        .write("app/models/forwarding_record.rb", parent)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ForwardingRecord",
        )
        .write("app/lib/keyword_forwarding_descendants.rb", descendants)
        .write("app/lib/probe.rb", probe)
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
        )
        .run_ruby("puts 'unsafe inherited keyword destinations are diagnosed'");
    assert!(run.success, "actual={}; stderr={}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "unsafe inherited keyword destinations are diagnosed\n");
    assert_eq!(run.errors.len(), 4, "errors={:?}", run.errors);
    assert!(
        run.errors.iter().all(|e| e.contains("model method synthesis")),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn keyword_packet_refuses_model_owned_contract_replaced_by_synthesis() {
    let probe = r#"
class Probe
  def self.relay(**)
    Article._conflict_predicate(**)
  end
end
"#;
    let native_source = r#"
class Article
  def self._conflict_predicate(**); :source; end
  def self._conflict_predicate(columns); :generated; end
end
class Probe
  def self.relay(**); Article._conflict_predicate(**); end
end
raise "model-owned source method unexpectedly survived synthesis" unless Probe.relay(value: 41) == :generated
puts "model-owned source method is replaced"
"#;
    let native = Command::new("ruby")
        .args(["-e", native_source])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "model-owned source method is replaced\n"
    );

    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def self._conflict_predicate(**); :source; end",
        )
        .write("app/lib/probe.rb", probe)
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.index [\"title\"], name: \"index_articles_live_title\", unique: true, where: \"(id > 0)\"",
        )
        .run_ruby("puts 'model-owned replacement is diagnosed'");
    assert!(run.success, "actual={}; stderr={}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "model-owned replacement is diagnosed\n");
    assert_eq!(run.errors.len(), 1, "errors={:?}", run.errors);
    assert!(
        run.errors[0].contains("model method synthesis"),
        "errors={:?}",
        run.errors
    );
}

#[test]
fn full_mixin_forwarder_refuses_an_ordinary_destination_shadowed_by_model_synthesis() {
    for (name, refused) in [("custom_title", false), ("title", true)] {
        let api = format!(
            "module TitleAPI; def relay(...); {name}(...); end; def {name}(a,b); 11; end; end"
        );
        let native = Command::new("ruby")
            .args([
                "-e",
                &format!(
                    "{api}; class Article; include TitleAPI; end; puts Article.new.relay(7,3)"
                ),
            ])
            .output()
            .unwrap();
        assert!(native.status.success());
        assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/title_api.rb", &api)
            .edit(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n include TitleAPI\n",
            )
            .write(
                "app/lib/probe.rb",
                "class Probe; def self.run; Article.new.relay(7,3); end; end",
            )
            .run_ruby("puts Probe.run");
        if refused {
            assert!(
                run.errors
                    .iter()
                    .any(|e| e.contains("model method synthesis")
                        || e.contains("unpreserved subclass contract")),
                "{:?}; {}",
                run.errors,
                run.stderr
            );
        } else {
            run.assert_passes();
            assert_eq!(run.stdout, "11\n");
        }
    }
}

#[test]
fn packet_constructor_checks_the_inherited_initializer_not_only_new() {
    let api = "module ConstructorAPI; def initialize(a,b); @proof=a-b; end; def proof; @proof; end; end";
    let probe = "class Probe; def self.run(...); article=Article.new(...); article.proof; end; end";
    let native = Command::new("ruby")
        .args([
            "-e",
            &format!("{api}; class Article; include ConstructorAPI; end; {probe}; puts Probe.run(11,3)"),
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "8\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/constructor_api.rb", api)
        .write("app/lib/probe.rb", probe)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n include ConstructorAPI\n",
        )
        .run_ruby("puts Probe.run(11,3)");
    // A selector-only survey would miss `initialize` behind `new(...)` and
    // admit the generated model constructor instead of the source contract.
    assert!(
        run.errors.iter().any(|e| e.contains("full argument forwarding")
            && e.contains("declaration cannot be verified")),
        "{:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn effective_ordinary_override_is_not_mistaken_for_an_inherited_full_collision() {
    let parent = "class ForwardingRecord < ApplicationRecord; self.abstract_class = true; def self.title(...); 11; end; end";
    let run = emit_and_run::real_blog()
        .write("app/models/forwarding_record.rb", parent)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ForwardingRecord\n def self.title(n); n+10; end\n",
        )
        .write(
            "app/lib/probe.rb",
            "class Probe; def self.run; Article.title(7); end; end",
        )
        .run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "17\n");
}

#[test]
fn inherited_ordinary_collisions_do_not_add_errors_without_packet_forwarding() {
    let run = emit_and_run::real_blog()
        .write(
            "app/lib/title_api.rb",
            "module TitleAPI; def title(a,b); 11; end; end",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n include TitleAPI\n",
        )
        // Activate the survey, but do not forward to the ordinary collision.
        .write(
            "app/lib/probe.rb",
            "class Probe; def self.unrelated(...); 41; end; end",
        )
        .run_ruby("puts 'loaded'");
    run.assert_passes();
    assert_eq!(run.stdout, "loaded\n");
}

#[test]
fn check_surveys_effective_model_declarations_without_lowering_or_diagnostic_leaks() {
    use roundhouse::analyze::{Analyzer, diagnose};
    use roundhouse::diagnostic::{Diagnostic, Severity};
    use roundhouse::emit::diagnostics::{push, scope};
    use roundhouse::ingest::ingest_app_from_tree;
    use roundhouse::span::Span;

    let permit_only = "class ArticlesController < ApplicationController; def article_params; params.require(:article).permit(:title); end; end";
    let create_demand = "class ArticlesController < ApplicationController; def create; @article=Article.create(article_params); end; def article_params; params.require(:article).permit(:title); end; end";
    // Same names and even identical `...` formals are not proof that the
    // effective source declaration survives. Receiver sides are independent;
    // controller-dependent factories must use the actual permit/demand survey.
    let cases: &[(&str, &str, &[bool])] = &[
        ("def self.call(n); 17; end", "", &[]),
        ("def self.call(...); 11; end", "", &[false]),
        (
            "def self.call(...); 11; end; def self.call(n); 17; end",
            "",
            &[true],
        ),
        (
            "def self.call(n); 17; end; def self.call(...); 29; end",
            "",
            &[true],
        ),
        (
            "def self.call(...); 11; end; def self.call(...); 29; end",
            "",
            &[true, true],
        ),
        (
            "def self.call(...); 11; end; def self.call(n); 17; end; def self.call(...); 29; end",
            "",
            &[true, true],
        ),
        (
            "def self.call(...); 11; end; def call(...); 29; end",
            "",
            &[false, false],
        ),
        (
            "def self.title(...); 11; end; def title(...); 29; end",
            "",
            &[false, true],
        ),
        ("def self.create_from_params(...); 11; end", "", &[false]),
        (
            "def self.create_from_params(...); 11; end",
            permit_only,
            &[false],
        ),
        (
            "def self.create_from_params(...); 11; end",
            create_demand,
            &[true],
        ),
    ];
    for (methods, controller, refused) in cases {
        // This unknown DSL statement makes synthesis emit a warning, allowing
        // the enclosing sink assertions to detect a survey that leaks it.
        let model =
            format!("class Article < ApplicationRecord\n unclaimed_macro :flag\n {methods}\nend");
        let tree = [
            ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do; create_table :articles do |t|; t.string :title; end; end"),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base; self.abstract_class = true; end"),
            ("app/models/article.rb", model.as_str()),
            ("app/controllers/articles_controller.rb", controller),
        ].into_iter().map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).unwrap();
        Analyzer::new(&app).analyze(&mut app);
        let spans: Vec<_> = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == "Article")
            .unwrap()
            .methods()
            .filter(|m| m.params.iter().any(|p| p.forwarding))
            .map(|m| m.name_span)
            .collect();
        assert_eq!(spans.len(), refused.len(), "{methods}");
        let before = Diagnostic::unsupported(Span::synthetic(), None, "before survey", "sentinel");
        let after = Diagnostic::unsupported(Span::synthetic(), None, "after survey", "sentinel");
        let (diags, emitted) = scope(|| {
            push(before.clone());
            let diags = diagnose(&app); // Deliberately no post-lowering pass.
            push(after.clone());
            diags
        });
        assert_eq!(emitted, vec![before, after], "{methods}");
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("model method synthesis"))
            .collect();
        assert_eq!(
            missing.len(),
            refused.iter().filter(|r| **r).count(),
            "{methods}: {diags:?}"
        );
        for (span, refused) in spans.iter().zip(*refused) {
            assert!(!span.is_synthetic());
            assert_eq!(
                missing
                    .iter()
                    .any(|d| d.span == *span && d.severity == Severity::Error),
                *refused,
                "{methods}: {diags:?}"
            );
        }
    }
}
