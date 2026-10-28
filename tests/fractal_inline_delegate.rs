//! `inline-delegate`: what it rewrites, what it declines and why, and (where
//! Ruby and Active Support's delegation are available) that the rewritten
//! class behaves as the original does.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::fractal::passes::InlineDelegate;
use roundhouse::emit::fractal::passes::inline_delegate::VERIFIED_ACTIVESUPPORT_VERSION;
use roundhouse::emit::fractal::{self, Pass, Plan};

const MODEL: &str = "app/models/order.rb";

fn lockfile(activesupport_version: &str) -> String {
    format!("GEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport ({activesupport_version})\n\nBUNDLED WITH\n   2.6.9\n")
}

fn app_directory(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let app = std::env::temp_dir().join(format!("roundhouse-fractal-inline-delegate-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&app);
    for (path, content) in files {
        let path = app.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    app
}

/// An app with `model` at `app/models/order.rb` and a lockfile resolving the
/// verified Active Support version.
fn app_with_model(name: &str, model: &str) -> PathBuf {
    app_directory(name, &[(MODEL, model), ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION))])
}

/// Plan `app/models/order.rb` with the pass the way the CLI does: ingest
/// what the scope decides, recording gaps (`--survey`) rather than stopping.
/// The pass must get to read the file, so a skipped file fails the test.
fn build_plan(app: &Path) -> Plan {
    let pass = InlineDelegate::default();
    let passes: [&dyn Pass; 1] = [&pass];
    let root = app.canonicalize().unwrap();
    let compiled = [MODEL.to_string()];
    let fractal::Ingest::Files { files, .. } = fractal::scope::decide(&root, &compiled, &passes, false).unwrap() else {
        panic!("inline-delegate needs no more than the compiled files")
    };
    let filesystem = roundhouse::vfs::FsVfs::new();
    let vfs = roundhouse::vfs::ScopedVfs::new(&filesystem, files);
    roundhouse::ingest::survey::activate();
    let ingested = roundhouse::ingest::app::ingest_app_with_vfs(&vfs, &root);
    let gaps = roundhouse::ingest::survey::drain();
    let mut ingested = ingested.unwrap();
    Analyzer::new(&ingested).analyze(&mut ingested);
    let untrusted: Vec<String> = gaps
        .iter()
        .filter_map(|gap| match gap {
            roundhouse::ingest::IngestError::Unsupported { file, .. } | roundhouse::ingest::IngestError::Parse { file, .. } => Some(file.clone()),
            _ => None,
        })
        .collect();
    let plan = fractal::plan(&ingested, &root, &compiled, &untrusted, &passes).unwrap();
    assert!(plan.report.skipped.is_empty(), "{:?}", plan.report.skipped);
    plan
}

/// The source Active Support 8.2 generates for `delegate :<method>, to: :<target>`.
fn generated(method: &str, target: &str) -> String {
    format!(
        "def {method}(...);  _ = {target};  _.{method}(...);rescue ::NoMethodError => e;  if _.nil? && e.name == :{method};    ::Kernel.raise ::ActiveSupport::DelegationError.nil_target(:{method}, :'{target}');  else;    ::Kernel.raise;  end;end"
    )
}

const SIMPLE: &str = r#"# frozen_string_literal: true

class Order
  delegate :name, :paid?, to: :shop
  delegate(:currency, to: :shop) # the currency the shop sells in
  delegate :domain,
    :email, # the shop's contact address
    to: :shop

  class << self
    delegate :default_shop, to: :registry
    def registry; end
  end

  def shop
    @shop
  end
end
"#;

#[test]
fn a_simple_delegate_becomes_the_methods_active_support_generates() {
    let app = app_with_model("simple", SIMPLE);
    let plan = build_plan(&app);
    assert!(plan.report.declined.is_empty(), "{:?}", plan.report.declined);
    assert_eq!(plan.report.rewrites.len(), 4, "{:?}", plan.report.rewrites);
    let expected = SIMPLE
        .replace("delegate :name, :paid?, to: :shop", &format!("{};{}", generated("name", "shop"), generated("paid?", "shop")))
        .replace(
            "delegate :domain,\n    :email, # the shop's contact address\n    to: :shop",
            &format!("{};{}\n\n", generated("domain", "shop"), generated("email", "shop")),
        )
        .replace("delegate(:currency, to: :shop)", &generated("currency", "shop"))
        .replace("delegate :default_shop, to: :registry", &generated("default_shop", "registry"));
    assert_eq!(plan.rewritten, [(MODEL.to_string(), expected.clone())]);
    assert_eq!(expected.lines().count(), SIMPLE.lines().count());
    assert_eq!(plan.report.rewrites[0].at, "app/models/order.rb:4:3");
    assert_eq!(plan.report.rewrites[0].pass, "inline-delegate");
}

#[test]
fn every_form_it_cannot_reproduce_exactly_is_declined_with_a_reason() {
    let cases: &[(&str, &str)] = &[
        ("delegate :name, to: :shop, prefix: true", "passes `prefix:`"),
        ("delegate :name, to: :shop, allow_nil: true", "passes `allow_nil:`"),
        ("delegate :name, to: :shop, private: true", "passes `private:`"),
        ("delegate :name, to: \"shop\"", "something other than a symbol literal"),
        ("delegate :name, to: :@shop", "not a plain method name"),
        ("delegate :name, to: :Shop", "not a plain method name"),
        ("delegate :name, to: :class", "not a plain method name"),
        ("delegate :name=, to: :shop", "a writer, an operator or a reserved name"),
        // Operators (`delegate :[], to: :shop`) are declined too, but ingest
        // records that call as a gap, so its file keeps the app's code before
        // any pass reads it; `plain_method_names` pins the pass's own check.
        ("delegate \"name\", to: :shop", "something other than a symbol literal"),
        ("delegate *NAMES, to: :shop", "something other than a symbol literal"),
        ("delegate :name, **OPTIONS", "splats its options"),
        ("delegate :name", "has no `to:` option"),
        ("delegate :name, to: :shop if ENABLED", "is not a direct statement"),
        ("included do\n    delegate :name, to: :shop\n  end", "is not a direct statement"),
        ("private\n  delegate :name, to: :shop", "may change this body's default visibility"),
        // `self.private`, `private if ...` and `private(*[])` change it too, but
        // ingest records those as gaps, so their files keep the app's code
        // before any pass reads them; the pass's unit tests pin them.
        ("send(:protected)\n  delegate :name, to: :shop", "may change this body's default visibility"),
        ("send(\"private\")\n  delegate :name, to: :shop", "may change this body's default visibility"),
        ("send(:using, Shouting)\n  delegate :name, to: :shop", "activates a refinement with `using`"),
        ("__send__(\"using\", Shouting)\n  delegate :name, to: :shop", "activates a refinement with `using`"),
        ("using Shouting\n  delegate :name, to: :shop", "activates a refinement with `using`"),
    ];
    for (statement, reason) in cases {
        let model = format!("class Order\n  {statement}\n\n  def shop; end\nend\n");
        let app = app_with_model("declined", &model);
        let plan = build_plan(&app);
        assert!(plan.rewritten.is_empty(), "{statement}: {:?}", plan.report.rewrites);
        assert_eq!(plan.report.declined.len(), 1, "{statement}: {:?}", plan.report.declined);
        assert_eq!(plan.report.declined[0].pass, "inline-delegate");
        assert!(plan.report.declined[0].reason.contains(reason), "{statement}: {}", plan.report.declined[0].reason);
    }
}

#[test]
fn a_delegate_whose_value_or_owner_is_uncertain_is_declined() {
    let cases: &[(&str, &str)] = &[
        // The class expression's value would change from the method names to the last one.
        ("class Order\n  def shop; end\n  delegate :name, to: :shop\nend\n", "is the last statement"),
        // A def at the top level defines a private method on Object.
        ("delegate :name, to: :shop\nclass Order; end\n", "is not a direct statement"),
        ("class << OTHER\n  delegate :name, to: :shop\n  def shop; end\nend\n", "is not a direct statement"),
        // Lexical refinements reach a def written in the file, not Active Support's module_eval.
        ("using Shouting\nclass Order\n  delegate :name, to: :shop\n  def shop; end\nend\n", "activates a refinement with `using`"),
        // A `private def` takes an argument, so it leaves the default alone.
        ("class Order\n  private def helper; end\n  delegate :name, to: :shop\n  def shop; end\nend\n", "REWRITTEN"),
    ];
    for (model, reason) in cases {
        let app = app_with_model("uncertain", model);
        let plan = build_plan(&app);
        if *reason == "REWRITTEN" {
            assert_eq!(plan.report.rewrites.len(), 1, "{model}: {:?}", plan.report.declined);
            continue;
        }
        assert!(plan.rewritten.is_empty(), "{model}");
        assert_eq!(plan.report.declined.len(), 1, "{model}: {:?}", plan.report.declined);
        assert!(plan.report.declined[0].reason.contains(reason), "{model}: {}", plan.report.declined[0].reason);
    }
}

#[test]
fn an_unverified_active_support_or_an_app_delegate_method_declines_everything() {
    let without_lockfile = app_directory("no-lockfile", &[(MODEL, SIMPLE)]);
    let older = app_directory("older-rails", &[(MODEL, SIMPLE), ("Gemfile.lock", &lockfile("8.1.0"))]);
    let own_delegate = app_directory(
        "own-delegate",
        &[
            (MODEL, SIMPLE),
            ("Gemfile.lock", &lockfile(VERIFIED_ACTIVESUPPORT_VERSION)),
            ("lib/delegation.rb", "class Module\n  def delegate(*names, to:); end\nend\n"),
        ],
    );
    for (app, reason) in [
        (without_lockfile, "does not resolve activesupport"),
        (older, "resolves activesupport 8.1.0"),
        (own_delegate, "the app defines a `delegate` method a class body could call instead of Active Support's (lib/delegation.rb:2)"),
    ] {
        let plan = build_plan(&app);
        assert!(plan.rewritten.is_empty(), "{reason}");
        assert_eq!(plan.report.declined.len(), 4, "{reason}: {:?}", plan.report.declined);
        assert!(plan.report.declined.iter().all(|decline| decline.reason.contains(reason)), "{reason}: {:?}", plan.report.declined);
    }
}

#[test]
fn a_file_without_delegate_reads_nothing_beyond_itself() {
    // No candidate, so the pass never asks the context for the lockfile or
    // the app's definitions; with neither present it still proposes nothing.
    let app = app_directory("no-delegate", &[(MODEL, "class Order\n  def name; end\nend\n")]);
    let plan = build_plan(&app);
    assert!(plan.rewritten.is_empty() && plan.report.declined.is_empty());
}

/// A model exercising every observable a delegated method has.
const RUNTIME_MODEL: &str = r#"class Order
  delegate :name, :paid?, to: :shop
  delegate :currency,
    :fail_inside,
    to: :shop

  class << self
    delegate :default_shop, to: :registry
    def registry = Registry.new
  end

  attr_accessor :shop
end
"#;

/// Load an `order.rb` and print what Ruby can observe about its delegated
/// methods: owner, visibility, parameters, arity, source line, results,
/// what a NoMethodError inside the target looks like, and the error for a
/// nil target.
const OBSERVE: &str = r#"
require "active_support/core_ext/module/delegation"
class Shop
  def name = "shop"
  def paid? = true
  def currency(code = "CAD", fallback: nil) = [code, fallback]
  def fail_inside = nil.missing_inside
end
class Registry
  def default_shop = "default"
end
load ARGV.fetch(0)
observed = []
[[Order, %i[name paid? currency fail_inside]], [Order.singleton_class, %i[default_shop]]].each do |owner, names|
  names.each do |name|
    method = owner.instance_method(name)
    observed << [name, method.owner.inspect, owner.public_method_defined?(name), method.parameters, method.arity, File.basename(method.source_location[0]), method.source_location[1]]
  end
end
order = Order.new
order.shop = Shop.new
observed << order.name << order.paid? << order.currency << order.currency("USD", fallback: "EUR") << Order.default_shop
begin
  order.fail_inside
rescue NoMethodError => error
  observed << [error.class, error.name, error.message]
end
order.shop = nil
begin
  order.name
rescue NoMethodError => error
  observed << [error.class, error.message]
end
puts observed.inspect
"#;

#[test]
fn the_rewritten_class_behaves_as_the_original() {
    // Needs Ruby and Active Support's delegation on the load path; the Nix
    // build has neither.
    let available = Command::new("ruby").args(["-e", "require 'active_support/core_ext/module/delegation'"]).output();
    if !available.is_ok_and(|output| output.status.success()) {
        eprintln!("ruby with active_support/core_ext/module/delegation is not available; skipping the runtime comparison");
        return;
    }
    let app = app_with_model("runtime", RUNTIME_MODEL);
    let plan = build_plan(&app);
    assert_eq!(plan.report.rewrites.len(), 3, "{:?}", plan.report.declined);
    assert_eq!(plan.rewritten[0].1.lines().count(), RUNTIME_MODEL.lines().count());
    let rewritten_directory = app.join("rewritten");
    fs::create_dir_all(&rewritten_directory).unwrap();
    fs::write(rewritten_directory.join("order.rb"), &plan.rewritten[0].1).unwrap();
    let script = app.join("observe.rb");
    fs::write(&script, OBSERVE).unwrap();
    let observe = |model: &Path| {
        let output = Command::new("ruby").arg(&script).arg(model).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let original = observe(&app.join(MODEL));
    let rewritten = observe(&rewritten_directory.join("order.rb"));
    assert_eq!(original, rewritten);
    assert!(original.contains("delegated to shop, but shop is nil"), "{original}");
}
