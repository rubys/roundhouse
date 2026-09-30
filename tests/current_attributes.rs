//! `class Current < ActiveSupport::CurrentAttributes` becomes a plain,
//! statically-resolvable class (`ingest::current_attributes`).
//!
//! Rails stacks three pieces of metaprogramming here — `attribute`
//! defining accessors into a generated module, class-level `Current.user`
//! arriving via `method_missing`, and an app's own writer reaching the
//! generated one through `super`. An emitted tree has none of it, and a
//! statically-resolved target cannot follow `method_missing` at all.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn current() -> String {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        (
            "app/models/current.rb",
            r#"class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user, :request

  delegate :host, :protocol, to: :request, prefix: true, allow_nil: true

  def session=(value)
    super(value)

    if value.present?
      self.user = session.user
    end
  end

  def account
    Account.first
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("current.rb"))
        .map(|f| f.content.clone())
        .expect("current.rb")
}

/// The base is gone — nothing in an emitted tree provides it.
#[test]
fn the_current_attributes_base_is_dropped() {
    let c = current();
    assert!(!c.contains("ActiveSupport::CurrentAttributes"), "{c}");
    assert!(c.contains("class Current\n"), "{c}");
}

/// `attribute :session, :user, :request` becomes real ivar accessors.
#[test]
fn declared_attributes_become_accessors() {
    let c = current();
    for m in ["def user\n    @user", "def user=(value)\n    @user = value", "def request\n    @request"] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}

/// The app writes its OWN `session=`, so only the reader is synthesized —
/// and its `super(value)` becomes the storage write it means, rather than
/// needing a module for `super` to find.
#[test]
fn an_app_written_writer_keeps_its_body_and_loses_super() {
    let c = current();
    assert!(!c.contains("super("), "no super survives:\n{c}");
    assert!(
        c.contains("def session=(value)\n    @session = value"),
        "super became the storage write:\n{c}"
    );
    assert_eq!(c.matches("def session=").count(), 1, "not double-defined:\n{c}");
}

/// `delegate … prefix: true, allow_nil: true` — the nil guard is the
/// whole point of `allow_nil`.
#[test]
fn delegate_expands_with_its_nil_guard() {
    let c = current();
    assert!(c.contains("def request_host"), "prefixed name:\n{c}");
    assert!(c.contains("@request.nil?"), "allow_nil guard:\n{c}");
}

/// The surface app code actually calls. Rails supplies these through
/// `method_missing`; a statically-resolved target needs them real —
/// including for a method the app defined itself (`account`).
#[test]
fn class_level_forwarders_exist_for_every_instance_method() {
    let c = current();
    for m in [
        "def self.user\n    Current.instance.user",
        "def self.user=(value)\n    Current.instance.user = value",
        "def self.account\n    Current.instance.account",
        "def self.request_host",
        "def self.instance",
        "def self.reset",
    ] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}

fn scoped_app(body: &str) -> roundhouse::App {
    let probe = format!("class ScopeProbe\n  def self.run\n    {body}\n  end\nend\n");
    ingest_app_from_tree(tree(&[
        ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n"),
        ("app/lib/scope_probe.rb", &probe),
    ])).expect("synthetic Current source")
}

#[test]
fn current_set_rejects_lost_bindings_and_block_conversion_at_the_source_site() {
    use roundhouse::ExprNode;
    for (body, lost, passed, reason) in [
        ("Current.set(user: 83) { |context = 47| context }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |(left, right)| left }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |context; hidden| context }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { _1 }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |context,| context }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |context, *| context }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |*| 31 }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |context, *rest, last| last }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |flag: 47| flag }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |**keywords| keywords }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |context, &block| block }", true, false, "bindings not preserved"),
        ("Current.set(user: 83) { |*rest| rest }", false, false, "rest and block parameters"),
        ("Current.set(user: 83, &:to_s)", false, true, "converted or forwarded block operands"),
        ("Current.set(user: 83, &->(context) { return 31 })", false, true, "converted or forwarded block operands"),
        ("Current.set(user: 83, &lambda { return 31 })", false, true, "converted or forwarded block operands"),
        ("Current.set(user: 83, &proc { return 31 })", false, true, "converted or forwarded block operands"),
        ("Current.set(user: 83) { [1].each(&->(item) { return 31 }); 47 }", false, false, "converted or forwarded inner blocks"),
        ("Current.set(user: 83) { [1].each(&lambda { return 31 }); 47 }", false, false, "converted or forwarded inner blocks"),
        ("Current.set(user: 83) { -> { return 31 }.call }", false, false, "nested closure result ownership"),
        ("Current.set(user: 83, user: 109) { 31 }", false, false, "duplicate literal attribute keys"),
        ("Current.set(typo: 83) { 31 }", false, false, "represented instance reader and writer"),
        ("Current.set(\"user?\" => 83) { 31 }", false, false, "represented instance reader and writer"),
    ] {
        // Older fork bases already decline Proc/lambda block operands
        // during ingestion. Retain those negative source controls rather
        // than backporting unrelated Proc support merely to build their IR.
        let parsed = ruby_prism::parse(body.as_bytes());
        let node = parsed.node().as_program_node().unwrap().statements().as_node();
        if let Err(error) = roundhouse::ingest::ingest_expr(&node, "scope.rb") {
            assert!(body.contains("&->") || body.contains("&lambda") || body.contains("&proc"), "unexpected ingestion refusal: {body}: {error}");
            assert!(error.to_string().contains("block-argument forms"), "{body}: {error}");
            continue;
        }
        let mut app = scoped_app(body);
        let probe = app.library_classes.iter().find(|c| c.name.0.as_str() == "ScopeProbe").unwrap();
        let expr = &probe.methods[0].body;
        let ExprNode::Send { block: Some(block), .. } = &*expr.node else { panic!("scope send: {body}") };
        let ExprNode::Lambda { has_unrepresented_bindings, from_block_pass, .. } = &*block.node else { panic!("scope block: {body}") };
        assert_eq!((*has_unrepresented_bindings, *from_block_pass), (lost, passed), "{body}");
        let encoded = serde_json::to_value(&block.node).unwrap();
        let decoded: ExprNode = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(*block.node, decoded, "source facts survive serialization: {body}");
        if !lost { assert!(encoded.get("has_unrepresented_bindings").is_none()); }
        if !passed { assert!(encoded.get("from_block_pass").is_none()); }
        let refusals = roundhouse::lower::current_set::source_refusals(&app);
        assert!(refusals.iter().any(|d| !d.span.is_synthetic() && d.message.contains(reason)), "{body}: {refusals:?}");
        let before = ruby::emit_expr(expr);
        let rejected = roundhouse::lower::current_set::apply_current_set_lowering(&mut app);
        assert!(!rejected.is_empty(), "{body}");
        let after = app.library_classes.iter().find(|c| c.name.0.as_str() == "ScopeProbe").unwrap();
        assert_eq!(ruby::emit_expr(&after.methods[0].body), before, "source is retained: {body}");
        let error = roundhouse::lower::current_set::guard_output(&app, "ruby").expect_err(body);
        assert!(error.contains("app/lib/scope_probe.rb") && error.contains(reason), "{error}");
    }
    let mut old: serde_json::Value = serde_json::to_value(&scoped_app("Current.set(user: 83) { 31 }").library_classes).unwrap();
    // Accepted source uses omitted/false flags, so old serialized IR
    // remains readable without opting into either reject-only fact.
    let roundtrip: Vec<roundhouse::dialect::LibraryClass> = serde_json::from_value(old.take()).unwrap();
    assert!(!roundtrip.is_empty());
}

#[test]
fn literal_current_set_output_gate_is_site_specific_and_survives_lowering() {
    use roundhouse::project::{BuildTarget, target_files};
    let mut app = scoped_app("Current.set(user: 83) { 31 }");
    for stage in ["source", "lowered"] {
        for &target in BuildTarget::TRANSPILE {
            if target == BuildTarget::Ruby {
                assert!(roundhouse::lower::current_set::guard_output(&app, target.as_str()).is_ok());
            } else {
                // A missing fixture path proves refusal precedes scaffold
                // reads and file assembly, not a later emit diagnostic.
                let error = target_files(&app, std::path::Path::new("/nonexistent-current-set-fixture"), target).expect_err(stage);
                assert!(error.contains("CurrentAttributes#set") && error.contains("app/lib/scope_probe.rb") && error.contains("--allow-unsupported"), "{stage} {target:?}: {error}");
            }
        }
        assert!(roundhouse::lower::current_set::guard_output(&app, "wasm").is_err());
        let refusal = roundhouse::project::spinel_base_files(&app, std::path::Path::new("/nonexistent-current-set-fixture"));
        assert!(refusal.is_err(), "{stage}: public Spinel assembly bypassed Current.set preflight");
        let error = refusal.unwrap_err();
        assert!(error.contains("CurrentAttributes#set") && error.contains("app/lib/scope_probe.rb"), "{error}");
        assert!(roundhouse::lower::current_set::guard_output(&app, "blog").is_ok());
        roundhouse::session::analyze_and_lower(&mut app);
    }
    for body in ["31", "Current.set(values) { 31 }", "Current.set(user: 83)", "Other.set(user: 83) { 31 }"] {
        let app = scoped_app(body);
        for &target in BuildTarget::TRANSPILE {
            assert!(roundhouse::lower::current_set::guard_output(&app, target.as_str()).is_ok(), "no affected literal site: {body} {target:?}");
        }
    }
    let overridden = ingest_app_from_tree(tree(&[
        ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n attribute :user\n def self.set(values); \"source override\"; end\nend\n"),
        ("app/lib/probe.rb", "class Probe\n def self.run; Current.set(user: 83) { while true; break \"inner\"; end }; end\nend\n"),
    ])).unwrap();
    assert!(roundhouse::lower::current_set::source_refusals(&overridden).is_empty());
    assert!(roundhouse::lower::current_set::guard_output(&overridden, "typescript").is_ok());
}

#[test]
fn allow_unsupported_cannot_bypass_current_set_fail_before_write() {
    use std::process::Command;
    let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-gates-{}", std::process::id()));
    let source = scratch.join("source");
    std::fs::create_dir_all(source.join("app/models")).unwrap();
    std::fs::create_dir_all(source.join("app/lib")).unwrap();
    std::fs::write(source.join("app/models/current.rb"), "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n").unwrap();
    for (target, body) in [
        ("ruby", "Current.set(user: 83) { |context; hidden| context }"),
        ("jruby", "Current.set(user: 83) { 31 }"),
        ("typescript", "Current.set(user: 83) { 31 }"),
        ("roda", "Current.set(user: 83) { 31 }"),
    ] {
        std::fs::write(source.join("app/lib/probe.rb"), format!("class Probe\n def self.run\n {body}\n end\nend\n")).unwrap();
        for allow in [false, true] {
            let output = scratch.join(format!("{target}-{allow}"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
            command.args(["--target", target]).arg(&source).arg("-o").arg(&output);
            if allow { command.arg("--allow-unsupported"); }
            let result = command.output().expect("CLI output boundary");
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{target} allow={allow}: {stderr}");
            assert!(stderr.contains("Current.set output refusal cannot be bypassed"), "{target} allow={allow}: {stderr}");
            assert!(!output.exists(), "{target} allow={allow}: created output before refusal");
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn current_set_refusals_survive_shared_lowerings_and_association_extensions() {
    use roundhouse::project::{BuildTarget, target_files};
    for body in [
        "Current.set(user: 83) { Article.create { |article| break \"inner\" }; 31 }",
        "Current.set(typo: 83) { 31 }",
    ] {
        let mut app = scoped_app(body);
        let diags = roundhouse::session::analyze_and_lower(&mut app);
        assert!(!diags.is_empty(), "{body}");
        let error = target_files(&app, std::path::Path::new("/nonexistent-current-set-fixture"), BuildTarget::Ruby).expect_err(body);
        assert!(error.contains("CurrentAttributes#set") && error.contains("--allow-unsupported"), "{error}");
        let probe = app.library_classes.iter().find(|c| c.name.0.as_str() == "ScopeProbe").unwrap();
        assert!(ruby::emit_expr(&probe.methods[0].body).contains("Current.set"), "refused source retained");
    }
    for body in ["Current.set(user: 83) { 31 }", "Current.set(user: 83) { |context; lost| context }"] {
        let model = format!("class Article < ApplicationRecord\n has_many :comments do\n def scoped\n {body}\n end\n end\nend\n");
        let mut app = ingest_app_from_tree(tree(&[
            ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n"),
            ("app/models/article.rb", &model),
        ])).unwrap();
        assert!(roundhouse::lower::current_set::guard_output(&app, "wasm").is_err(), "extension source: {body}");
        assert!(roundhouse::lower::current_set::guard_output(&app, "roda").is_err(), "extension source: {body}");
        roundhouse::session::analyze_and_lower(&mut app);
        if body.contains("lost") {
            assert!(roundhouse::lower::current_set::guard_output(&app, "ruby").is_err());
        } else {
            assert!(roundhouse::lower::current_set::guard_output(&app, "typescript").is_err());
        }
    }
}

#[test]
fn current_set_site_preflight_preserves_existing_destination() {
    let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-site-{}", std::process::id()));
    let source = scratch.join("source");
    std::fs::create_dir_all(source.join("app/models")).unwrap();
    std::fs::create_dir_all(source.join("app/lib")).unwrap();
    std::fs::write(source.join("app/models/current.rb"), "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n").unwrap();
    std::fs::write(source.join("app/lib/probe.rb"), "class Probe\n def self.run; Current.set(user: 83) { 31 }; end\nend\n").unwrap();
    for exists in [false, true] {
        let out = scratch.join(format!("output-{exists}"));
        if exists {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("sentinel"), "previous site").unwrap();
        }
        let error = roundhouse::project::build_site(&source, &out).expect_err("unverified target refuses before site mutation");
        assert!(error.contains("CurrentAttributes#set"), "{error}");
        if exists {
            assert_eq!(std::fs::read_to_string(out.join("sentinel")).unwrap(), "previous site");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        } else {
            assert!(!out.exists());
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn namespaced_current_set_preflight_resolves_identity_without_mutating_source() {
    const CURRENT: &str = "module ScopeContext\n class Current < ActiveSupport::CurrentAttributes\n attribute :user\n end\nend\n";
    const PROBE: &str = "module ScopeContext\n class Probe\n def self.run\n Current.set(user: 83) { 31 }\n end\n end\nend\n";
    let app = ingest_app_from_tree(tree(&[
        ("app/models/current.rb", CURRENT), ("app/lib/probe.rb", PROBE),
    ])).unwrap();
    let before = app.clone();
    roundhouse::ingest::survey::activate();
    let started = std::time::Instant::now();
    for target in ["ruby", "typescript", "roda", "wasm"] {
        let result = roundhouse::lower::current_set::guard_output(&app, target);
        if target == "ruby" {
            result.unwrap();
        } else {
            let error = result.expect_err("raw bare reference resolves to namespaced Current");
            assert!(error.contains("app/lib/probe.rb") && error.contains("CurrentAttributes#set"), "{error}");
        }
    }
    println!("four isolated raw-identity preflights: {:?}", started.elapsed());
    assert!(roundhouse::ingest::survey::drain().is_empty(), "isolated analysis must not publish ingest gaps");
    assert_eq!(app, before, "gate must preserve original source/IR for Roda");

    let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-namespace-{}", std::process::id()));
    let source = scratch.join("source");
    std::fs::create_dir_all(source.join("app/models")).unwrap();
    std::fs::create_dir_all(source.join("app/lib")).unwrap();
    std::fs::write(source.join("app/models/current.rb"), CURRENT).unwrap();
    std::fs::write(source.join("app/lib/probe.rb"), PROBE).unwrap();
    for exists in [false, true] {
        let out = scratch.join(format!("output-{exists}"));
        if exists {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("sentinel"), "previous site").unwrap();
        }
        let error = roundhouse::project::build_site(&source, &out).expect_err("qualified scope refuses before site mutation");
        assert!(error.contains("CurrentAttributes#set"), "{error}");
        for (binary, output_flag, target) in [
            (env!("CARGO_BIN_EXE_emit_preview"), "--out", "typescript"),
            (env!("CARGO_BIN_EXE_roundhouse"), "-o", "roda"),
        ] {
            let result = std::process::Command::new(binary).args(["--target", target]).arg(&source).arg(output_flag).arg(&out).output().unwrap();
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success() && error.contains("CurrentAttributes#set"), "{error}");
        }
        if exists {
            assert_eq!(std::fs::read_to_string(out.join("sentinel")).unwrap(), "previous site");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        } else {
            assert!(!out.exists());
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn current_set_namespace_collisions_require_explicit_identity() {
    const CLASSES: &str = r#"
class Current < ActiveSupport::CurrentAttributes
  attribute :user
end
module FirstScope
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
end
module SecondScope
  class Current < ActiveSupport::CurrentAttributes
    attribute :user
  end
end
module OrdinaryScope
  class Current
    def self.set(values)
      "ordinary override"
    end
  end
end
"#;
    const PROBE: &str = r#"
module FirstScope
  class Probe
    def self.bare
      Current.set(user: 181) { Current.user }
    end
    def self.absolute
      ::FirstScope::Current.set(user: 197) { ::FirstScope::Current.user }
    end
    def self.global
      ::Current.set(user: 317) { ::Current.user }
    end
    def self.ordinary
      ::OrdinaryScope::Current.set(user: 421) { 67 }
    end
  end
end
"#;
    // Native identities are independently observable: bare lookup inside
    // FirstScope reaches that scope, not top-level or SecondScope.
    let native = std::process::Command::new("ruby").args(["-e", &format!(
        "gem 'activesupport', '8.1.4'\nrequire 'active_support'\nrequire 'active_support/current_attributes'\n{CLASSES}\n{PROBE}\nCurrent.user = 47\nFirstScope::Current.user = 83\nSecondScope::Current.user = 151\np [FirstScope::Probe.bare, FirstScope::Probe.absolute, FirstScope::Probe.global, FirstScope::Probe.ordinary, Current.user, FirstScope::Current.user, SecondScope::Current.user]"
    )]).output().unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "[181, 197, 317, \"ordinary override\", 47, 83, 151]");
    let app = ingest_app_from_tree(tree(&[
        ("app/models/current.rb", CLASSES), ("app/lib/probe.rb", PROBE),
    ])).unwrap();
    let mut analyzed = app.clone();
    roundhouse::analyze::Analyzer::new(&analyzed).analyze(&mut analyzed);
    let probe = analyzed.library_classes.iter().find(|c| c.name.0.as_str() == "FirstScope::Probe").unwrap();
    for method in &probe.methods {
        if let roundhouse::ExprNode::Send { recv: Some(recv), .. } = &*method.body.node {
            println!("{} receiver: {:?} {:?}", method.name, recv.node, recv.ty);
        }
    }
    for source in [&app, &analyzed] {
        let error = roundhouse::lower::current_set::guard_output(source, "ruby").expect_err("bare collision must not be accepted using the wrong Current identity");
        assert!(error.contains("ambiguous") && error.contains("app/lib/probe.rb"), "{error}");
    }
    // Explicit absolute identities retain their exact class and override
    // contracts; removing ONLY the ambiguous method makes Ruby admitted.
    let explicit = PROBE.replace("    def self.bare\n      Current.set(user: 181) { Current.user }\n    end\n", "");
    let explicit = ingest_app_from_tree(tree(&[
        ("app/models/current.rb", CLASSES), ("app/lib/probe.rb", &explicit),
    ])).unwrap();
    roundhouse::lower::current_set::guard_output(&explicit, "ruby").unwrap();
    assert!(roundhouse::lower::current_set::guard_output(&explicit, "typescript").is_err());
}

#[test]
fn untyped_namespaced_extension_scope_requires_explicit_identity() {
    const CURRENT: &str = "module ScopeContext\n class Current < ActiveSupport::CurrentAttributes\n attribute :user\n end\nend\n";
    for body in ["Current.set(user: 83) { 31 }", "Current.set(user: 83) { |context; lost| context }"] {
        let model = format!("module ScopeContext\n class Article < ApplicationRecord\n has_many :comments do\n def scoped\n {body}\n end\n end\n end\nend\n");
        let mut app = ingest_app_from_tree(tree(&[
            ("app/models/current.rb", CURRENT),
            ("app/models/article.rb", &model),
        ])).unwrap();
        for stage in ["raw", "post-lowering"] {
            for target in ["ruby", "roda", "wasm", "typescript"] {
                let error = roundhouse::lower::current_set::guard_output(&app, target).expect_err(stage);
                assert!(error.contains("explicit qualification") && error.contains("app/models/article.rb"), "{error}");
            }
            let diags = roundhouse::session::analyze_and_lower(&mut app);
            assert!(diags.iter().any(|d| d.message.contains("explicit qualification")));
        }
        let explicit_model = model.replace("Current.set", "::ScopeContext::Current.set");
        let explicit = ingest_app_from_tree(tree(&[
            ("app/models/current.rb", CURRENT),
            ("app/models/article.rb", &explicit_model),
        ])).unwrap();
        if body.contains("lost") {
            let error = roundhouse::lower::current_set::guard_output(&explicit, "ruby").unwrap_err();
            assert!(error.contains("bindings not preserved"));
        } else {
            roundhouse::lower::current_set::guard_output(&explicit, "ruby").unwrap();
        }
        assert!(roundhouse::lower::current_set::guard_output(&explicit, "wasm").is_err());
        let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-extension-site-{}", std::process::id()));
        let source = scratch.join("source");
        std::fs::create_dir_all(source.join("app/models")).unwrap();
        std::fs::write(source.join("app/models/current.rb"), CURRENT).unwrap();
        std::fs::write(source.join("app/models/article.rb"), &model).unwrap();
        for exists in [false, true] {
            let out = scratch.join(format!("out-{exists}"));
            if exists {
                std::fs::create_dir_all(&out).unwrap();
                std::fs::write(out.join("sentinel"), "previous site").unwrap();
            }
            let error = roundhouse::project::build_site(&source, &out).expect_err("untyped extension refuses before site mutation");
            assert!(error.contains("explicit qualification"), "{error}");
            if exists {
                assert_eq!(std::fs::read_to_string(out.join("sentinel")).unwrap(), "previous site");
                assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
            } else {
                assert!(!out.exists());
            }
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn retained_block_return_context_does_not_leak_into_independent_programs() {
    fn ingest(source: &str) -> Result<roundhouse::Expr, roundhouse::ingest::IngestError> {
        let parsed = ruby_prism::parse(source.as_bytes());
        assert!(parsed.errors().next().is_none(), "valid Ruby control");
        let statements = parsed.node().as_program_node().unwrap().statements().as_node();
        roundhouse::ingest::ingest_expr(&statements, "db/seeds.rb")
    }
    fn return_count(expr: &roundhouse::Expr) -> usize {
        let mut count = usize::from(matches!(&*expr.node, roundhouse::ExprNode::Return { .. }));
        expr.node.for_each_child(&mut |child| count += return_count(child));
        count
    }
    for source in [
        "Current.set(user: 83) { return if true; 31 }",
        "Current.set(user: 83) { Current.set(user: 109) { return if true; 31 }; return if false; 99 }",
        "Current.set(user: 83) { work(&@callback) }",
    ] {
        let block = ingest(source);
        if source.contains("@callback") {
            assert!(block.is_err(), "error unwind control");
        } else {
            let expected = if source.contains("109") { 2 } else { 1 };
            assert_eq!(return_count(&block.unwrap()), expected, "retain each actual exit");
        }
        // A new parse on the SAME thread must retain the ordinary seed
        // guard rewrite after either success, nested entry or an error.
        let program = ingest("return if true; 31").unwrap();
        assert_eq!(return_count(&program), 0);
        assert!(matches!(&*program.node, roundhouse::ExprNode::If { then_branch, .. }
            if matches!(&*then_branch.node, roundhouse::ExprNode::Lit { value: roundhouse::Literal::Nil })));
    }
}

#[test]
fn unresolved_relative_qualified_current_set_cannot_bypass_preflight() {
    const CURRENT: &str = "module Outer\n module Tenant\n class Current < ActiveSupport::CurrentAttributes\n attribute :user\n end\n end\nend\n";
    const PROBE: &str = "module Outer\n class Probe\n def self.run\n Tenant::Current.set(user: 83) { 31 }\n end\n end\nend\n";
    let native = std::process::Command::new("ruby").args(["-e", &format!(
        "gem 'activesupport', '8.1.4'\nrequire 'active_support'\nrequire 'active_support/current_attributes'\n{CURRENT}\n{PROBE}\nOuter::Tenant::Current.user = 47\np [Outer::Probe.run, Outer::Tenant::Current.user]"
    )]).output().unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "[31, 47]");
    let app = ingest_app_from_tree(tree(&[("app/models/current.rb", CURRENT), ("app/lib/probe.rb", PROBE)])).unwrap();
    let mut analyzed = app.clone();
    roundhouse::analyze::Analyzer::new(&analyzed).analyze(&mut analyzed);
    for source in [&app, &analyzed] {
        let before = source.clone();
        for target in ["ruby", "typescript", "roda", "wasm"] {
            let error = roundhouse::lower::current_set::guard_output(source, target).expect_err("unproven relative class identity");
            assert!(error.contains("explicit qualification") && error.contains("app/lib/probe.rb"), "{error}");
        }
        assert_eq!(*source, before);
    }
    let absolute = PROBE.replace("Tenant::Current.set", "::Outer::Tenant::Current.set");
    let absolute = ingest_app_from_tree(tree(&[("app/models/current.rb", CURRENT), ("app/lib/probe.rb", &absolute)])).unwrap();
    roundhouse::lower::current_set::guard_output(&absolute, "ruby").unwrap();

    let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-relative-{}", std::process::id()));
    let source = scratch.join("source");
    std::fs::create_dir_all(source.join("app/models")).unwrap();
    std::fs::create_dir_all(source.join("app/lib")).unwrap();
    std::fs::write(source.join("app/models/current.rb"), CURRENT).unwrap();
    std::fs::write(source.join("app/lib/probe.rb"), PROBE).unwrap();
    for exists in [false, true] {
        let out = scratch.join(format!("out-{exists}"));
        if exists {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("sentinel"), "previous site").unwrap();
        }
        for target in ["ruby", "typescript", "roda"] {
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
                .args(["--target", target, "--allow-unsupported"]).arg(&source).arg("-o").arg(&out).output().unwrap();
            assert!(!result.status.success() && String::from_utf8_lossy(&result.stderr).contains("explicit qualification"));
        }
        let error = roundhouse::project::build_site(&source, &out).expect_err("refuse before site mutation");
        assert!(error.contains("explicit qualification"), "{error}");
        if exists {
            assert_eq!(std::fs::read_to_string(out.join("sentinel")).unwrap(), "previous site");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        } else { assert!(!out.exists()); }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn current_set_unlowered_source_containers_are_refused() {
    const CURRENT: &str = "class Current < ActiveSupport::CurrentAttributes\n attribute :user\nend\n";
    for (path, source) in [
        ("config/routes.rb", "Rails.application.routes.draw do\n direct :scoped do\n ::Current.set(user: 83) { '/scoped' }\n end\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n belongs_to :user, default: -> { ::Current.set(user: 83) { 31 } }\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n has_many :users, -> { ::Current.set(user: 83) { 31 } }\nend\n"),
        ("test/models/probe_test.rb", "class ProbeTest < ActiveSupport::TestCase\n VALUE = ::Current.set(user: 83) { 31 }\nend\n"),
        ("test/models/probe_test.rb", "class ProbeTest < ActiveSupport::TestCase\n class Inner\n def run; ::Current.set(user: 83) { 31 }; end\n end\nend\n"),
        ("test/models/probe_test.rb", "class ProbeTest < ActiveSupport::TestCase\n def helper(value = ::Current.set(user: 83) { 31 }); value; end\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n def index(marker: ::Current.set(user: 83) { 31 }); marker; end\nend\n"),
        ("test/fixtures/articles.yml", "<% marker = ::Current.set(user: 83) { 31 } %>\none:\n  title: scoped\n"),
        ("test/fixtures/articles.yml", "one:\n  title: <%= ::Current.set(user: 83) { 'scoped' } %>\n"),
    ] {
        let mut app = ingest_app_from_tree(tree(&[("app/models/current.rb", CURRENT), (path, source)])).unwrap();
        let before = app.clone();
        for target in ["ruby", "typescript", "roda", "wasm"] {
            let error = roundhouse::lower::current_set::guard_output(&app, target).expect_err(path);
            assert!(error.contains(path) && error.contains("source container"), "{error}");
        }
        assert_eq!(app, before, "raw source is preserved");
        let diags = roundhouse::session::analyze_and_lower(&mut app);
        assert!(diags.iter().any(|d| d.message.contains("source container")), "{diags:?}");
        let error = roundhouse::project::target_files(&app, std::path::Path::new("/nonexistent-current-set-fixture"), roundhouse::project::BuildTarget::Ruby).expect_err(path);
        assert!(error.contains(path) && error.contains("source container"), "{error}");
    }
    // The header parser currently retains only literal defaults. Pin
    // that existing source limit separately from this represented-IR
    // admission boundary; do not advertise rich header expressions.
    let mut view_app = ingest_app_from_tree(tree(&[
        ("app/models/current.rb", CURRENT),
        ("app/lib/scope_probe.rb", "class ScopeProbe\n def self.run; ::Current.set(user: 83) { 31 }; end\nend\n"),
        ("app/views/articles/_probe.html.erb", "<%# locals: (marker: ::Current.set(user: 83) { 31 }) %>\n<%= marker %>\n"),
    ])).unwrap();
    let params = view_app.views[0].strict_locals.as_mut().unwrap();
    assert!(matches!(&*params[0].default.as_ref().unwrap().node, roundhouse::ExprNode::Lit { value: roundhouse::Literal::Sym { value } } if value.as_str() == ":Current.set(user: 83) { 31 }"));
    let scope = view_app.library_classes.iter().find(|class| class.name.0.as_str() == "ScopeProbe").unwrap().methods[0].body.clone();
    params[0].default = Some(scope);
    let error = roundhouse::lower::current_set::guard_output(&view_app, "ruby").expect_err("represented strict-local default");
    assert!(error.contains("source container") && error.contains("app/lib/scope_probe.rb"), "{error}");
    // Direct routes bypassed the first policy traversal entirely. The
    // hard refusal must also precede both single-target and site writes.
    let scratch = std::env::temp_dir().join(format!("roundhouse-current-set-direct-{}", std::process::id()));
    let source = scratch.join("source");
    std::fs::create_dir_all(source.join("app/models")).unwrap();
    std::fs::create_dir_all(source.join("config")).unwrap();
    std::fs::write(source.join("app/models/current.rb"), CURRENT).unwrap();
    std::fs::write(source.join("config/routes.rb"), "Rails.application.routes.draw do\n direct :scoped do\n ::Current.set(user: 83) { '/scoped' }\n end\nend\n").unwrap();
    for exists in [false, true] {
        let output = scratch.join(format!("output-{exists}"));
        if exists {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("sentinel"), "previous output").unwrap();
        }
        for target in ["ruby", "roda", "typescript"] {
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
                .args(["--target", target, "--allow-unsupported"]).arg(&source).arg("-o").arg(&output).output().unwrap();
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success() && error.contains("source container"), "{error}");
        }
        let error = roundhouse::project::build_site(&source, &output).expect_err("direct helper site preflight");
        assert!(error.contains("source container"), "{error}");
        if exists {
            assert_eq!(std::fs::read_to_string(output.join("sentinel")).unwrap(), "previous output");
            assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
        } else {
            assert!(!output.exists());
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
