#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

fn native_result(source: &str, script: &str, expected: &str) {
    let output = Command::new("ruby")
        .args(["-e", &format!("{source}; {script}")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn compiled_keyword_producers_preserve_identity_order_and_single_evaluation() {
    let source = r#"
class Counter
  def initialize; @calls = 0; end
  def produce(factor); @calls += 1; {factor: factor, offset: @calls}; end
  def calls; @calls; end
end
class Probe
  def self.target(a,b,factor:,offset: 0,**,&block)
    result = (a-b)*factor+offset
    block ? block.call(result) : result
  end
  def self.call(...); target(...); end
  def self.run
    kw = {factor: 3}
    Probe.call(11,4,**kw)
  end
  def self.positional
    kw = {factor: 3}
    begin
      Probe.call(11,4,kw)
    rescue ArgumentError
      "positional"
    end
  end
  def self.mixed
    counter = Counter.new
    result = Probe.call(11,4,offset: 6,**counter.produce(5),factor: 7,**counter.produce(3))
    result*10+counter.calls
  end
  def self.transformed
    kw = {factor: 3}
    Probe.call(11,4,**kw) { |r| r*2+1 }
  end
end
"#;
    // Without keyword provenance run is an arity error; treating a positional
    // hash as keywords changes positional; duplication/order errors change mixed.
    let script = "puts Probe.run; puts Probe.positional; puts Probe.mixed; puts Probe.transformed";
    let expected = "21\npositional\n232\n43\n";
    native_result(source, script, expected);
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/probe.rb")).unwrap();
    assert!(emitted.contains("Probe.call(11, 4, **kw)"), "{emitted}");
    assert!(emitted.contains("Probe.call(11, 4, kw)"), "{emitted}");
}

#[test]
fn keyword_producer_projection_keeps_ordinary_lowering_and_super_contracts() {
    let source = r#"
class Parent
  def call(...); 11; end
end
class Child < Parent
  def call; kw = {factor: 3}; super(7,**kw); end
end
class Sink
  def target(a,b,factor:); (a-b)*factor; end
end
class Probe
  def self.run; helper=Sink.new; kw={factor:3}; helper.target(11,4,**kw); end
  def self.hash; kw={factor:3}; {**kw}; end
end
"#;
    let script = "puts Probe.run; puts Child.new.call; puts Probe.hash[:factor]";
    native_result(source, script, "21\n11\n3\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "21\n11\n3\n");
    let child = std::fs::read_to_string(run.emitted.join("app/models/child.rb")).unwrap();
    assert!(child.contains("super(7, **kw)"), "{child}");
    let ordinary = std::fs::read_to_string(run.emitted.join("app/models/probe.rb")).unwrap();
    // Ordinary source keywords keep the native producer. Flattening
    // `**kw` into a positional hash would change Ruby's binding.
    assert!(ordinary.contains("helper.target(11, 4, **kw)"),
        "{ordinary}"
    );
}

#[test]
fn instance_keyword_normalization_uses_the_effective_last_definition() {
    let flat = "def target(factor: 2); factor*11; end";
    let full = "def target(...); Leaf.accept(...); end";
    for (first, last, expected) in [(flat, full, "21\n"), (full, flat, "33\n")] {
        let source = format!(
            "class Sink; {first}; {last}; end; class Leaf; def self.accept(factor:); factor*7; end; end; class Probe; def self.run; sink=Sink.new; sink.target(factor:3); end; end"
        );
        let script = "puts Probe.run";
        native_result(&source, script, expected);
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", &source)
            .run_ruby(script);
        run.assert_passes();
        assert_eq!(run.stdout, expected);
    }
}

#[test]
fn unknown_ordinary_super_abi_is_not_mislabeled_as_full_forwarding() {
    for (extra, relevant_full_selector) in [
        ("", false),
        ("class Other; def unrelated(...); 11; end; end", false),
        ("class Other; def initialize(...); end; end", true),
    ] {
        let source = format!("class Probe < StandardError; def initialize; options={{}}; super(**options); end; end; {extra}");
        let script = "puts Probe.new.message";
        native_result(&source, script, "Probe\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", &source)
            .run_ruby(script);
        if relevant_full_selector {
            assert!(
                run.errors.iter().any(|e| e.contains("keyword producer")),
                "{:?}; actual={}; stderr={}",
                run.errors,
                run.stdout,
                run.stderr
            );
        } else {
            // Simply restoring Legacy would emit super(options), changing
            // Ruby's empty-keyword semantics: the message becomes "{}".
            assert!(run.errors.iter().any(|e| e.contains("ordinary super") && e.contains("argument ABI")), "{:?}", run.errors);
            assert!(!run.errors.iter().any(|e| e.contains("full forwarding")), "{:?}", run.errors);
        }
    }
}

#[test]
fn source_super_keyword_rest_keeps_verified_legacy_projection() {
    for (options, expected) in [("{}", "8\n"), ("{left:33,right:11}", "22\n")] {
        let source = format!("class Parent; def initialize(**options); @proof=options.fetch(:left,11)-options.fetch(:right,3); end; def proof; @proof; end; end; class Probe < Parent; def initialize; options={options}; super(**options); end; end");
        let script = "puts Probe.new.proof";
        native_result(&source, script, expected);
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", &source)
            .run_ruby(script);
        run.assert_passes();
        assert_eq!(run.stdout, expected);
    }
}

#[test]
fn compiled_keyword_producer_into_forwarding_operator_keeps_call_syntax() {
    let source = "class Sink; def ==(...); 11; end; end; class Probe; def self.run; kw={factor:3}; Sink.new.==(**kw); end; end";
    native_result(source, "puts Probe.run", "11\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "11\n");
}

#[test]
fn ordinary_keyword_producers_keep_ingest_rewrites_and_call_parentheses() {
    let source = "class Probe; def self.accept(options={}); options[:factor]; end; def self.parens; Probe.accept **{factor:3}; end; def self.reverse; options={factor:7}; defaults={factor:3}; options.reverse_merge(**defaults)[:factor]; end; end";
    let script = "puts Probe.parens; puts Probe.reverse";
    let native = Command::new("ruby")
        .args([
            "-ractive_support/core_ext/hash/reverse_merge",
            "-e",
            &format!("{source};{script}"),
        ])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n7\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "3\n7\n");
}

#[test]
fn ordinary_exists_keyword_conditions_keep_the_query_rewrite() {
    let source = "class Probe; def self.present(id); Article.exists?(**{id:id}); end; end";
    let native_source = format!(
        "require 'active_record'; ActiveRecord::Base.establish_connection(adapter:'sqlite3',database:':memory:'); ActiveRecord::Schema.define {{ create_table :articles }}; class Article < ActiveRecord::Base; end; {source}; a=Article.create!; puts Probe.present(a.id); puts Probe.present(a.id+11)"
    );
    let native = Command::new("ruby")
        .args(["-e", &native_source])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert!(String::from_utf8_lossy(&native.stdout).ends_with("true\nfalse\n"));
    let run = emit_and_run::real_blog().write("app/lib/probe.rb", source)
        .run_ruby("a=Article.new; a.title='present'; a.body='long enough article body'; a.save; puts Probe.present(a.id); puts Probe.present(a.id+11)");
    run.assert_passes();
    assert_eq!(run.stdout, "true\nfalse\n");
}

#[test]
fn forwarding_packet_is_not_used_as_a_reverse_merge_receiver() {
    let source = "class Sink; def reverse_merge(a,b,factor:); (a-b)*factor; end; end; class Probe; def self.call(...); Sink.new.reverse_merge(...); end; end";
    native_result(source, "puts Probe.call(11,4,factor:3)", "21\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby("puts Probe.call(11,4,factor:3)");
    run.assert_passes();
    assert_eq!(run.stdout, "21\n");
}

#[test]
fn full_declaration_with_an_erased_ingest_selector_is_refused() {
    for source in [
        "class Probe; def self.reverse_merge(...); 11; end; def self.run; Probe.reverse_merge(**{factor:3}); end; end",
        "class ExistsResult; def exists?; 7; end; end; class Probe; def self.exists?(...); 11; end; def self.where(options); ExistsResult.new; end; def self.run; Probe.exists?(**{factor:3}); end; end",
    ] {
        native_result(source, "puts Probe.run", "11\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", source)
            .run_ruby("puts Probe.run");
        assert!(
            run.errors.iter().any(|e| e.contains("ingest-time")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn compiled_empty_and_optional_keyword_packets_survive_two_hops() {
    let source = "class Sink; def self.scale(n,factor:2,**); n*factor; end; end; class Probe; def self.call(...); Sink.scale(...); end; def self.bridge(...); call(...); end; def self.empty; kw={}; Probe.bridge(7,**kw); end; def self.present; kw={factor:3}; Probe.bridge(7,**kw); end; end";
    let script = "puts Probe.empty; puts Probe.present";
    native_result(source, script, "14\n21\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby(script);
    run.assert_passes();
    assert_eq!(run.stdout, "14\n21\n");
}

#[test]
fn competing_ordinary_and_full_virtual_keyword_contracts_execute_native_declarations() {
    let source = "class Sink; def self.leaf(factor:,extra:); factor-extra; end; end; class Parent; def run; kw={factor:11,extra:4}; target(**kw); end; def target(factor:); factor; end; end; class Child < Parent; def target(...); Sink.leaf(...); end; end";
    native_result(source, "puts Child.new.run", "7\n");
    let child = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby("puts Child.new.run");
    child.assert_passes();
    assert_eq!(child.stdout, "7\n");
    // The parent's own declaration still rejects the extra key. Native
    // retention must not turn that ArgumentError into a successful call.
    let parent = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby("begin; Parent.new.run; rescue ArgumentError; puts 'unknown-extra-key'; end");
    parent.assert_passes();
    assert_eq!(parent.stdout, "unknown-extra-key\n"
    );
}

#[test]
fn keyword_producer_modifier_keeps_operand_grouping() {
    let source = "class Probe; def self.accept(...); 11; end; def self.run; kw={factor:3}; accept(**(kw rescue {factor:7})); end; end";
    native_result(source, "puts Probe.run", "11\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", source)
        .run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "11\n");
}

#[test]
fn carrier_keyword_packets_check_real_singletons_and_every_includer() {
    for carrier in [
        "class << self; def build(**fields); new(**fields); end; end",
        "module ClassMethods; def build(**fields); new(**fields); end; end; def self.included(base); base.extend(ClassMethods); end",
    ] {
        let source = format!(r#"
module Factory
  {carrier}
end
class Product
  include Factory
  def initialize(label:); @label=label; end
  def label; @label; end
end
class Other
  include Factory
  def initialize(attributes={{}}); @label=attributes[:label]; end
  def label; @label; end
end
class Probe
  def self.run; Product.build(label: "native").label; end
end
"#);
        // A real singleton's `new` does not run on an includer. A template
        // does, but every copied body must be checked, not just Product's.
        let script = if carrier.starts_with("class << self") {
            "begin; Factory.build(label: 'singleton'); rescue NoMethodError; puts 'no-new'; end"
        } else {
            "puts Probe.run; puts Other.build(label: 'hash').label"
        };
        native_result(&source, script, if carrier.starts_with("class << self") {
            "no-new\n"
        } else {
            "native\nhash\n"
        });
        let run = emit_and_run::real_blog().write("app/lib/probe.rb", &source).run_ruby(script);
        assert!(run.errors.iter().any(|e| e.contains("keyword") && e.contains("cannot be verified")),
            "a surviving copy must not hide an unverified singleton/includer: {:?}", run.errors);
    }
    let source = "module Factory; module ClassMethods; def build(**fields); new(**fields); end; end; def self.included(base); base.extend(ClassMethods); end; end; class Product; include Factory; def initialize(label:); @label=label; end; def label; @label; end; end; class Probe; def self.run; options={label:'x'}; begin; Factory.build(**options).label; rescue NoMethodError => error; error.name.to_s; end; end; end";
    native_result(source, "puts Probe.run", "build\n");
    let run = emit_and_run::real_blog().write("app/lib/probe.rb", source).run_ruby("puts Probe.run");
    assert!(run.errors.iter().any(|e| e.contains("keyword destination's native argument ABI cannot be verified")),
        "a template must not invent a callable Factory.build: {:?}", run.errors);
    let copy = source.replace("Factory.build(**options)", "Product.build(**options)");
    native_result(&copy, "puts Probe.run", "x\n");
    let run = emit_and_run::real_blog().write("app/lib/probe.rb", &copy).run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "x\n");
}
