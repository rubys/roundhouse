//! `defined?(Const)` is answered at compile time when the tree, the
//! lockfile and config/application.rb decide it, and only then.
//!
//! The strict targets refuse a runtime `defined?`; folding the provable
//! guards (EngineeredAt: 84 of them) must never turn an unprovable one
//! into a silent `nil`. A wrong `false` disables app behaviour, so the
//! unfolded cases below are as load-bearing as the folded ones.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_expr;
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM
  remote: https://rubygems.org/
  specs:
    activestorage (8.1.0)
    pagy (9.0.0)
    rails (8.1.0)
    sidekiq-cron (1.0.0)
    solid_queue (1.0.0)
    turbo-rails (2.0.0)

PLATFORMS
  ruby

DEPENDENCIES
  pagy
  rails
  solid_queue
  turbo-rails
";

const APPLICATION: &str = "require_relative \"boot\"\nrequire \"rails/all\"\nmodule Shop\n  class Application < Rails::Application\n  end\nend\n";

const PROBE: &str = r#"class Probe
  def rails_const; defined?(Rails); end
  def framework; defined?(ActiveStorage); end
  def framework_rooted; defined?(::ActiveStorage); end
  def tree_class; defined?(Widget::Part); end
  def tree_rooted; defined?(::Widget); end
  def modeled_gem; defined?(Turbo::StreamsChannel); end
  def modeled_gem_member; defined?(SolidQueue::Job); end
  def absent_gem; defined?(ActsAsTenant); end
  def absent_rooted; defined?(::Refer); end
  def absent_nested; defined?(MiniMagick::Image); end
  def prefix_only; defined?(Widget::Nope); end
  def gem_in_lock; defined?(Pagy); end
  def transitive_gem_prefix; defined?(Sidekiq); end
  def gem_outside_catalog_member; defined?(SolidQueue::Weird); end
  def initializer_defined; defined?(Throttle); end
  def runtime_provided; defined?(Set); end
  def ivar; defined?(@memo); end
  def local; defined?(thing); end
  def method_call; defined?(Rails.logger); end
  def guard_false; x = 1; x += 1 if defined?(ActsAsTenant); x; end
  def guard_true; x = 1; x += 1 if defined?(ActiveStorage); x; end
  def early_return; return :out unless defined?(::Widget); :in; end
  def conjunction; defined?(Rails) && Rails.respond_to?(:logger) && Rails.logger; end
  def disabled_conjunction; defined?(ActsAsTenant) && ActsAsTenant.current; end
  def negated; !defined?(ActsAsTenant); end
  def compacted; [:a, (:b if defined?(OmniAuth))].compact; end
  def value_of_true; v = defined?(Widget); v; end
end
"#;

fn ingest(lock: Option<&str>) -> roundhouse::App {
    let mut files: Vec<(&str, &str)> = vec![
        ("config/application.rb", APPLICATION),
        ("lib/probe.rb", PROBE),
        ("lib/widget.rb", "module Widget\nend\n"),
        ("lib/widget/part.rb", "module Widget\n  class Part\n  end\nend\n"),
        (
            "app/views/widgets/_badge.html.erb",
            "<% if defined? Refer %>referral<% end %><% if defined?(ActiveStorage) %>storage<% end %>",
        ),
        // Defined by an initializer nothing names, so the tree drops it.
        ("config/initializers/throttle.rb", "Throttle = Struct.new(:limit)\n"),
    ];
    if let Some(lock) = lock {
        files.push(("Gemfile.lock", lock));
    }
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::lower::defined_const_fold::apply_defined_const_fold(&mut app);
    app
}

fn bodies(app: &roundhouse::App) -> HashMap<String, String> {
    let probe = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Probe")
        .expect("Probe is ingested");
    probe.methods.iter().map(|m| (m.name.as_str().to_string(), emit_expr(&m.body))).collect()
}

#[test]
fn provable_constants_fold_to_true() {
    let b = bodies(&ingest(Some(LOCK)));
    for name in [
        "rails_const", "framework", "framework_rooted", "tree_class", "tree_rooted",
        "modeled_gem", "modeled_gem_member",
    ] {
        assert_eq!(b[name], "\"constant\"", "{name}");
    }
    // Ruby's value for a defined constant is the string, not `true`.
    assert_eq!(b["value_of_true"], "v = \"constant\"\nv");
}

#[test]
fn constants_no_locked_gem_can_define_fold_to_false() {
    let b = bodies(&ingest(Some(LOCK)));
    for name in ["absent_gem", "absent_rooted", "absent_nested"] {
        assert_eq!(b[name], "nil", "{name}");
    }
}

#[test]
fn undecidable_guards_stay_defined() {
    let b = bodies(&ingest(Some(LOCK)));
    // Prefix resolves, tail unknown: a gem or reopening may add it.
    assert_eq!(b["prefix_only"], "defined?(Widget::Nope)");
    // A gem in the lock outside the catalog: maybe `require: false`.
    assert_eq!(b["gem_in_lock"], "defined?(Pagy)");
    assert_eq!(b["transitive_gem_prefix"], "defined?(Sidekiq)");
    assert_eq!(b["gem_outside_catalog_member"], "defined?(SolidQueue::Weird)");
    // Defined by an initializer the tree dropped: not provably absent.
    assert_eq!(b["initializer_defined"], "defined?(Throttle)");
    // Needs a `require` the tree cannot show.
    assert_eq!(b["runtime_provided"], "defined?(Set)");
    // Not constants.
    assert_eq!(b["ivar"], "defined?(@memo)");
    assert_eq!(b["local"], "defined?(thing)");
    assert_eq!(b["method_call"], "defined?(Rails.logger)");
}

#[test]
fn without_a_lockfile_nothing_folds_false() {
    let b = bodies(&ingest(None));
    assert_eq!(b["absent_gem"], "defined?(ActsAsTenant)");
    assert_eq!(b["absent_rooted"], "defined?(::Refer)");
    // Tree and application.rb facts do not need the lock.
    assert_eq!(b["tree_class"], "\"constant\"");
    assert_eq!(b["framework"], "\"constant\"");
    // The catalog gem needs the lock to be known.
    assert_eq!(b["modeled_gem"], "defined?(Turbo::StreamsChannel)");
}

#[test]
fn folded_guards_prune_their_dead_branch() {
    let b = bodies(&ingest(Some(LOCK)));
    let dead = &b["guard_false"];
    assert!(!dead.contains("defined?") && !dead.contains("x += 1"), "{dead}");
    let live = &b["guard_true"];
    assert!(!live.contains("defined?") && !live.contains(" if "), "{live}");
    assert!(live.contains("x += 1") || live.contains("x = x + 1"), "{live}");
    let early = &b["early_return"];
    assert!(!early.contains("return") && !early.contains("defined?"), "{early}");
    // `true && a && b` keeps the value of `a && b`; `false && x` is nil.
    let conj = &b["conjunction"];
    assert!(!conj.contains("defined?") && conj.contains("Rails.respond_to?(:logger)"), "{conj}");
    assert_eq!(b["disabled_conjunction"], "nil");
    assert_eq!(b["negated"], "true");
    assert!(!b["compacted"].contains("OmniAuth") && !b["compacted"].contains(":b"), "{}", b["compacted"]);
}

#[test]
fn templates_fold_too() {
    let app = ingest(Some(LOCK));
    let view = app.views.iter().find(|v| v.name.as_str().contains("badge")).expect("badge view");
    let body = emit_expr(&view.body);
    assert!(!body.contains("defined?"), "{body}");
    assert!(!body.contains("referral"), "{body}");
    assert!(body.contains("storage"), "{body}");
}
