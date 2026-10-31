//! Methods, accessors and hooks a gem defines under a load-time conditional
//! are the ones of the branch Ruby takes.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;
use std::process::Command;

const LAZY: &str = "Rails.application.configure do\n  config.eager_load = false\nend\n";

#[test]
fn the_branch_ruby_selects_at_load_defines_the_method() {
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", LAZY)
        .write(
            "lib/sampling.rb",
            r#"module Sampling
  RATE = 0.0001

  class << self
    if Rails.env.test?
      #: () -> String
      def diagnostic_sampling = "test"
    else
      #: () -> String
      def diagnostic_sampling
        RATE > 1 ? "sampled" : "other"
      end
    end
  end
end
"#,
        )
        .write(
            "lib/cache_store.rb",
            r#"class CacheStore
  if Rails.env.production?
    def clear
      "production"
    end
  elsif Rails.env.development?
    def clear
      "development"
    end
  else
    # Nothing clears the cache in production.
  end

  private

  unless Rails.env.development?
    def verify = "strict"
  else
    def verify = "lenient"
  end

  public

  # Every branch defines `mode`, so the change applies whichever ran.
  if Rails.env.production?
    def mode = "production"
  else
    def mode = "other"
  end
  private :mode

  def report
    [clear, verify, mode, CacheStore.private_method_defined?(:verify), CacheStore.public_method_defined?(:clear),
     CacheStore.private_method_defined?(:mode)]
  end
end
"#,
        )
        .run_ruby("p Sampling.diagnostic_sampling\np CacheStore.new.report\n");
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "\"other\"\n[\"development\", \"lenient\", \"other\", true, true, true]\n");
}

#[test]
fn an_accessor_in_the_branch_ruby_selects_at_load_is_defined() {
    let run = emit_and_run::empty_app()
        .write(
            "lib/connection.rb",
            r#"require "net/http"

class Connection
  attr_accessor :endpoint

  if Net::HTTP.instance_methods.include?(:min_version=)
    attr_accessor :min_version
    attr_accessor :max_version
  else
    attr_reader :legacy
  end

  unless Net::HTTP.instance_methods.include?(:min_version=)
    attr_writer :never
  end

  private

  if Net::HTTP.method_defined?(:start)
    attr_reader :secret
  end

  public

  def initialize
    @secret = "s"
  end

  def report
    self.min_version = :TLS1_2
    [min_version, max_version, respond_to?(:legacy), respond_to?(:never=), respond_to?(:secret), secret]
  end
end
"#,
        )
        .run_ruby("p Connection.new.report\n");
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "[:TLS1_2, nil, false, false, false, \"s\"]\n");
}

fn app(root: &Path, files: &[(&str, &str)]) {
    let _ = std::fs::remove_dir_all(root);
    for (path, text) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ]
    .iter()
    .chain(files)
    {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

fn check(root: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(root).output().unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_branch_that_changes_visibility_stays_refused_by_name() {
    let root = std::env::temp_dir().join(format!("roundhouse-conditional-definitions-{}", std::process::id()));
    app(
        &root,
        &[
            (
                "lib/marked.rb",
                "class Marked\n  if Rails.env.test?\n    private\n\n    def helper = 1\n  else\n    def helper = 2\n  end\nend\n",
            ),
            (
                "lib/accessor.rb",
                "class Accessor\n  if Rails.env.test?\n    def helper = 1\n    alias_method :token, :helper\n  end\nend\n",
            ),
            ("lib/plain.rb", "class Plain\n  if Rails.env.test?\n    def helper = 1\n  end\nend\n"),
            // Ruby raises NameError at `private :helper` when the branch
            // that defines it did not run.
            (
                "lib/partial.rb",
                "class Partial\n  if Rails.env.test?\n    def helper = 1\n  end\n  private :helper\nend\n",
            ),
        ],
    );
    let text = check(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        text.contains("lib/marked.rb:3:5: error[ingest_unsupported]: a visibility declaration inside a load-time \
                       conditional definition is not modeled"),
        "{text}"
    );
    assert!(
        text.contains("lib/accessor.rb:4:5: error[ingest_unsupported]: an accessor or alias declared in a load-time \
                       conditional definition is not modeled"),
        "{text}"
    );
    assert!(
        text.contains("lib/partial.rb:5:3: error[ingest_unsupported]: visibility change for `helper`, which a \
                       load-time conditional defines in only some branches, is not modeled"),
        "{text}"
    );
    assert!(!text.contains("lib/plain.rb"), "{text}");
}

#[test]
#[ignore = "fork spec: conditional declarations"]
fn a_carrier_modules_extended_hook_runs_when_it_extends_an_includer() {
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", LAZY)
        .write(
            "lib/sharded.rb",
            r#"module Sharded
  class << self
    def included(base)
      base.extend(ClassMethods)
    end
  end

  module ClassMethods
    class << self
      def extended(base)
        base.instance_variable_set(:@sharded_hook, "extended #{base.name}")
      end
    end

    def sharded_table?
      true
    end

    def sharded_hook
      @sharded_hook
    end
  end
end
"#,
        )
        // The same hook on an ActiveSupport::Concern's carrier.
        .write(
            "lib/tracked.rb",
            r#"module Tracked
  extend ActiveSupport::Concern

  module ClassMethods
    class << self
      def extended(base)
        base.instance_variable_set(:@tracked_hook, "tracked #{base.name}")
      end
    end

    def tracked_hook
      @tracked_hook
    end
  end
end
"#,
        )
        .write("lib/widget.rb", "class Widget\n  include Sharded\n  include Tracked\nend\n")
        // A model includer.
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"shops\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/shop.rb", "class Shop < ApplicationRecord\n  include Sharded\n  include Tracked\nend\n")
        .run_ruby(
            "p [Widget.sharded_table?, Widget.sharded_hook, Widget.tracked_hook]\n\
             p [Shop.sharded_table?, Shop.sharded_hook, Shop.tracked_hook]\n\
             p Sharded::ClassMethods.singleton_methods\n",
        );
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(
        run.stdout,
        "[true, \"extended Widget\", \"tracked Widget\"]\n[true, \"extended Shop\", \"tracked Shop\"]\n[:extended]\n"
    );
}

#[test]
fn a_conditional_nested_in_a_load_time_branch_runs_as_ruby_runs_it() {
    const GEM: &str = r#"module Vc
  LEVEL = Gem::Version.new("8.2.0")

  if LEVEL >= Gem::Version.new("7.1")
    def outer = "7.1+"

    if LEVEL >= Gem::Version.new("9.0")
      def inner = "9+"
    elsif LEVEL >= Gem::Version.new("8.1.0.alpha")
      def inner = "8.1+"

      unless LEVEL >= Gem::Version.new("8.2")
        def deepest = "pre-8.2"
      else
        def deepest = "8.2+"
      end
    else
      def inner = "pre-8.1"
    end

    def after = "after nested"
  elsif LEVEL >= Gem::Version.new("6.0")
    def outer = "6.x"

    if LEVEL >= Gem::Version.new("6.1")
      def inner = "6.1"
    end
  else
    def outer = "old"
  end

  unless LEVEL >= Gem::Version.new("7.0")
    if LEVEL >= Gem::Version.new("6.0")
      def legacy = "legacy"
    end
  end

  def report
    [outer, inner, deepest, after, respond_to?(:legacy), Vc.private_method_defined?(:secret), secret]
  end

  private

  if LEVEL >= Gem::Version.new("7.0")
    if LEVEL.segments.first == 8
      def secret = "s"
    else
      def secret = "other"
    end
  end
end
"#;
    const WIDGET: &str = "class Widget\n  include Vc\nend\n";
    const SCRIPT: &str = "p Widget.new.report\n";
    const EXPECTED: &str = "[\"7.1+\", \"8.1+\", \"8.2+\", \"after nested\", false, true, \"s\"]\n";

    let root = std::env::temp_dir().join(format!("roundhouse-nested-conditional-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(root.join("lib/vc.rb"), GEM).unwrap();
    std::fs::write(root.join("widget.rb"), WIDGET).unwrap();
    let native = emit_and_run::ruby()
        .arg("-I")
        .arg(root.join("lib"))
        .args(["-r", "vc", "-r"])
        .arg(root.join("widget.rb"))
        .args(["-e", SCRIPT])
        .output()
        .expect("CRuby control");
    let _ = std::fs::remove_dir_all(&root);
    assert!(native.status.success(), "native control failed:\n{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED, "native control");

    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", LAZY)
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "PATH\n  remote: vendor/gems/vc-1.0.0\n  specs:\n    vc (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  vc!\n")
        .write("config/application.rb", "require \"vc\"\n")
        .write("vendor/gems/vc-1.0.0/lib/vc.rb", GEM)
        .write("lib/widget.rb", WIDGET)
        .run_ruby(SCRIPT);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
}
