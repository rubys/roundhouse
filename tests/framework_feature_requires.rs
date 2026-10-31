//! PostgreSQL: framework feature requires, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
//
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

const PROBE_GEM: &str = r#"require "active_support"
require "active_model"
require "active_record"
require "action_controller"
require "action_mailer"
require "active_job"
require "active_support/core_ext/object/blank"
require "active_support/multibyte/unicode"
require "active_support/core_ext/module/attribute_accessors_per_thread"
# gems' sub-feature requires.
require "active_support/concern"
require "active_support/core_ext/object/try"
require "active_support/core_ext/hash/keys"
require "active_support/lazy_load_hooks"
require "active_support/notifications"
require "active_support/logger"
require "action_controller/metal/exceptions"
require "action_view"
require "active_support/core_ext/string/strip"
# activeresource 6.2.0's base.rb.
require "active_support/core_ext/kernel/reporting"
require "active_support/core_ext/module/attribute_accessors.rb"
require "active_support/core_ext/class/attribute_accessors"
require "active_support/core_ext/class"
require "active_support/core_ext/class/attribute"
require "active_support/core_ext/module/redefine_method"
require "active_support/core_ext/module/remove_method"
require "active_support/core_ext/numeric"
require "active_support/number_helper"

require "rails/engine"

module ProbeGem
  ROUTES_RAN = []

  # graphql 2.6.11's GraphQL::Dashboard shape: an engine whose routes
  # are appended at class load and drawn only when the routes load.
  class Dashboard < Rails::Engine
    engine_name "probe_gem_dashboard"
    isolate_namespace(ProbeGem::Dashboard)

    routes do
      ROUTES_RAN << :drawn
      root "landings#show"
    end
  end

  # turbo-rails 2.x Turbo::Current's shape.
  class Current
    thread_mattr_accessor :request_id
    thread_mattr_accessor :level, default: :info
  end

  # tailwindcss-rails 4.1.0's and sassc-rails 2.1.2's generator defaults.
  class Railtie < Rails::Railtie
    config.app_generators do |g|
      g.template_engine :tailwindcss
    end
    config.app_generators.stylesheet_engine :sass
    config.app_generators.test_unit fixture: false, aliases: { f: :fixture }
  end

  # softer_delete's shape: a concern with an `included` block, class
  # methods and a concern it depends on.
  module Stamped
    extend ActiveSupport::Concern

    included do
      attr_accessor :stamp
    end

    class_methods do
      def stamped? = true
    end
  end

  module SoftDeletable
    extend ActiveSupport::Concern
    include Stamped

    module ClassMethods
      def deleted_scope = (name.split("::").last.downcase + "_deleted").to_sym
    end

    included do
      self.deletable_columns = [:deleted_at]
    end

    prepended do
      @prepended_into = true
    end
  end

  class Record
    class << self
      attr_accessor :deletable_columns
    end
    include SoftDeletable
  end

  class PrependedRecord
    class << self
      attr_accessor :deletable_columns
    end
    prepend SoftDeletable
  end

  ActiveSupport.on_load(:probe_gem_hook) { HOOKS << [:ran, self] }
  HOOKS = []

  def self.concern
    twice = begin
      Module.new do
        extend ActiveSupport::Concern
        included { 1 }
        included { 2 }
      end
    rescue ActiveSupport::Concern::MultipleIncludedBlocks => e
      e.message
    end
    r = Record.new
    r.stamp = :s
    [Record.ancestors.take(3).map(&:name), Record.stamped?, Record.deleted_scope, Record.deletable_columns, r.stamp,
     PrependedRecord.ancestors.take(3).map(&:name), PrependedRecord.deleted_scope, PrependedRecord.instance_variable_get(:@prepended_into), twice]
  end

  def self.try
    [nil.try(:length), "abc".try(:length), "abc".try(:nope), "abc".try { |s| s.upcase }, nil.try!(:length),
     begin; "abc".try!(:nope); rescue NoMethodError => e; e.name; end]
  end

  def self.hash_keys
    h = { a: { b: [{ c: 1 }] }, "d" => 2, 3 => :x }
    bang = { a: 1, "b" => 2 }
    bang.stringify_keys!
    invalid = begin
      { a: 1, z: 2 }.assert_valid_keys(:a, [:b])
    rescue ArgumentError => e
      e.message
    end
    [h.stringify_keys, h.symbolize_keys, h.deep_stringify_keys, h.deep_symbolize_keys, h.to_options, bang,
     h.deep_transform_keys(&:to_s), { a: 1 }.assert_valid_keys(:a, :b), invalid]
  end

  # attributes.
  module Settings
    mattr_accessor :level, default: :info
    mattr_reader(:built) { [:block] }
    mattr_writer :sink, instance_writer: false
    cattr_accessor :hidden, instance_accessor: false
  end

  class Holder
    include Settings
    cattr_accessor :count, default: 0
  end

  def self.mattr
    Settings.level = :warn
    Holder.count += 2
    h = Holder.new
    invalid = begin
      Module.new { mattr_accessor :"bad-name" }
    rescue NameError => e
      e.message
    end
    singleton = begin
      Class.new.singleton_class.mattr_accessor(:x)
    rescue TypeError => e
      e.message
    end
    Settings.sink = :io
    [Settings.level, h.level, Settings.built, h.built, Settings.class_variable_get(:@@sink), h.respond_to?(:sink=),
     Settings.respond_to?(:hidden), h.respond_to?(:hidden), Holder.count, h.count, (h.count = 5; Holder.count), invalid, singleton,
     File.basename(Settings.instance_method(:level).source_location.first), File.basename(Holder.method(:count).source_location.first)]
  end

  # activerecord-rescue_from_duplicate's and app_profiler's class
  # attributes.
  class AttrBase
    class_attribute :setting, default: :base
    class_attribute :tags, instance_writer: false, default: []
    class_attribute :secret, instance_accessor: false, instance_predicate: false
  end

  class AttrChild < AttrBase; end

  class AttrGrandChild < AttrChild; end

  def self.class_ext
    AttrChild.setting = :child
    b = AttrBase.new
    c = AttrChild.new
    c.setting = :instance
    invalid = begin
      Class.new { class_attribute 1 }
    rescue TypeError => e
      e.message
    end
    single = Object.new
    single.singleton_class.class_attribute :solo, default: :s
    redefined = Class.new do
      def x = 1
      private def y = 2
    end
    redefined.redefine_method(:x) { 10 }
    redefined.redefine_method(:y) { 20 }
    removable = Class.new do
      def x = 1
      private def y = 2
      def self.z = 3
    end
    removable.remove_possible_method(:x)
    removable.remove_possible_method(:y)
    removable.remove_possible_method(:never_defined)
    removable.remove_possible_singleton_method(:z)
    removed = [removable.method_defined?(:x), removable.private_method_defined?(:y), removable.respond_to?(:z)]
    [AttrBase.setting, AttrChild.setting, AttrGrandChild.setting, b.setting, c.setting, AttrChild.new.setting, AttrBase.setting?, b.setting?,
     b.respond_to?(:tags=), AttrBase.tags, AttrBase.respond_to?(:secret), b.respond_to?(:secret), b.respond_to?(:secret?), AttrBase.respond_to?(:secret?),
     invalid, single.solo, AttrBase.descendants.map(&:name).sort, AttrBase.subclasses.map(&:name), AttrGrandChild.descendants,
     redefined.new.x, redefined.private_method_defined?(:y), redefined.new.send(:y), redefined.method_visibility(:y),
     ActiveSupport::DescendantsTracker.descendants(AttrBase).size, removed]
  end

  # number helpers Numeric#to_fs formats through.
  def self.numeric
    nh = ActiveSupport::NumberHelper
    [12.kilobytes, 1.megabyte, 2.5.gigabytes, 3.bytes, Numeric::PETABYTE,
     2.days.to_i, 1.5.hours.to_i, 3.weeks.to_i, 1.fortnight.to_i, 90.seconds.to_i, 2.minutes.to_i, 2.days == 172_800, 2.days.class.name,
     5.in_milliseconds, (Time.at(0).utc + 1.day).to_i, 1.hour.from_now > Time.now,
     1234567.to_fs(:delimited), 1234567.891.to_fs(:rounded, precision: 2), 0.5.to_fs(:percentage), 1234.5.to_fs(:currency),
     1234567.to_fs(:human), 12.kilobytes.to_fs(:human_size), 5551234.to_fs(:phone, area_code: true), 42.to_fs, 255.to_fs(16),
     3.to_fs(:unknown), 1.5.to_formatted_s(:rounded), BigDecimal("1.50").to_s, BigDecimal("1.50").to_fs(:currency),
     nh.number_to_currency(-1234.5, unit: "EUR", format: "%n %u"), nh.number_to_human(1_500_000, precision: 2),
     nh.number_to_human(0.001, units: { unit: "m", mili: "mm" }), nh.number_to_delimited("12345678.05", delimiter: "."),
     nh.number_to_rounded(111.2345, significant: true, precision: 2), nh.number_to_human_size(1_234_567_890),
     nh.number_to_phone(1235551234, country_code: 1, extension: 55), nh.number_to_percentage(302.24398923423, precision: 5, strip_insignificant_zeros: true),
     nh.number_to_currency("abc"), nh.number_to_rounded(nil), nh.number_to_delimited(Float::INFINITY)]
  end

  def self.strip
    usage = <<-EOM
      This command does such and such.

      Supported options are:
        -h         This message
    EOM
    frozen = "  a\n    b\n".freeze
    [usage.strip_heredoc, "x\n  y".strip_heredoc, "".strip_heredoc, "  a\n\n  b\r\n".strip_heredoc,
     frozen.strip_heredoc, frozen.strip_heredoc.frozen?, "  a".dup.strip_heredoc.frozen?, "\t\tq\n\t r".strip_heredoc,
     "  \n    z".strip_heredoc]
  end

  # activeresource 6.2.0's `prefix=` redefines its readers in
  # `silence_warnings`.
  def self.reporting
    verbose = $VERBOSE
    seen = []
    silence_warnings { seen << $VERBOSE }
    enable_warnings { seen << $VERBOSE }
    with_warnings(false) { seen << $VERBOSE }
    raised = begin
      silence_warnings { raise ArgumentError, "inside" }
    rescue ArgumentError => e
      [e.message, $VERBOSE == verbose]
    end
    suppressed = suppress(ZeroDivisionError, KeyError) { 1 / 0 }
    other = begin
      suppress(KeyError) { raise IndexError, "kept" }
    rescue IndexError => e
      e.message
    end
    [seen, $VERBOSE == verbose, raised, suppressed, other, suppress { :value }, Kernel.respond_to?(:silence_warnings, true)]
  end

  def self.notifications
    events = []
    subscriber = ActiveSupport::Notifications.subscribe("probe_gem.ran") { |event| events << [event.name, event.payload] }
    result = ActiveSupport::Notifications.instrument("probe_gem.ran", id: 7) { :done }
    ActiveSupport::Notifications.unsubscribe(subscriber)
    ActiveSupport::Notifications.instrument("probe_gem.ran", id: 8)
    [result, events]
  end

  def self.logger
    io = StringIO.new
    logger = ActiveSupport::Logger.new(io)
    logger.info("hello")
    logger.debug { "lazy" }
    logger.level = :warn
    logger.info("dropped")
    logger.silence { logger.error("silenced") }
    logger.warn("kept")
    [ActiveSupport::Logger.ancestors.include?(::Logger), io.string]
  end

  def self.controller_errors
    ac = ActionController
    chain = ->(klass) { klass.ancestors.take_while { |k| k != StandardError }.map(&:name) }
    routing = ac::RoutingError.new("No route", [:x])
    [chain[ac::BadRequest], chain[ac::RoutingError], chain[ac::UnknownFormat], chain[ac::MissingExactTemplate], chain[ac::NotImplemented],
     ac::BadRequest.new("Invalid parameter: x").message, ac::MethodNotAllowed.new(:get, :post, :put).message,
     ac::SessionOverflowError.new.message.split(".").first, ac::RespondToMismatchError.new.message.split(".").first,
     [routing.message, routing.failures], ac::MissingExactTemplate.new("m", :c, :a).then { |e| [e.message, e.controller, e.action_name] },
     ac::UrlGenerationError.new("u", nil, :r, :m).then { |e| [e.routes, e.route_name, e.method_name] },
     [ac::RenderError, ac::MissingFile, ac::UnknownHttpMethod, ac::TooManyRequests].map { |k| k.superclass.name },
     ActionView::MissingTemplate < StandardError]
  end

  def self.report
    unicode = ActiveSupport::Multibyte::Unicode
    latin1 = "caf\xE9".dup.force_encoding(Encoding::UTF_8)
    {
      frameworks: [ActiveRecord::Base, ActionController::Base, ActionMailer::Base, ActiveJob::Base, ActiveModel::Type::Value].map(&:name),
      blank: [nil.blank?, "  ".blank?, [].present?, { a: 1 }.present?],
      unicode_version: unicode::UNICODE_VERSION,
      tidy_bytes: unicode.tidy_bytes(latin1),
      tidy_bytes_valid: unicode.tidy_bytes("café"),
      tidy_bytes_forced: unicode.tidy_bytes("caf\xE9".b, true),
      compose: unicode.compose("é".codepoints),
      decompose: unicode.decompose(:canonical, "é".codepoints),
      decompose_compatibility: unicode.decompose(:compatibility, "ﬁ".codepoints),
      transliterate: ActiveSupport::Inflector.transliterate(latin1),
      parameterize: "Ærøskøbing Straße".parameterize,
      per_thread: per_thread,
      app_generators: generators,
      engine: engine,
      concern: concern,
      try: try,
      hash_keys: hash_keys,
      strip: strip,
      reporting: reporting,
      mattr: mattr,
      class_ext: class_ext,
      numeric: numeric,
      load_hooks: (ActiveSupport.run_load_hooks(:probe_gem_hook, :base); HOOKS),
      notifications: notifications,
      logger: logger,
      controller_errors: controller_errors,
    }
  end

  def self.engine
    d = Dashboard
    isolated = [:railtie_namespace, :table_name_prefix, :use_relative_model_naming?, :railtie_helpers_paths, :railtie_routes_url_helpers]
    [d.engine_name, d.isolated?, d.instance.engine_name, isolated.map { |m| d.respond_to?(m) }, d.routes.equal?(d.instance.routes), d.instance.routes?.nil?, ROUTES_RAN]
  end

  def self.generators
    g = Rails::Railtie::Configuration.new.app_generators
    [g.class.name, g.options[:rails].slice(:template_engine, :stylesheet_engine), g.options[:test_unit], g.aliases[:test_unit], g.template_engine]
  end

  def self.per_thread
    Current.request_id = "main"
    other = Thread.new { [Current.request_id, Current.level, (Current.new.request_id = "t"), Current.request_id] }.value
    invalid = begin
      Class.new { thread_mattr_accessor :"bad-name" }
    rescue NameError => e
      e.message
    end
    [Current.request_id, Current.new.request_id, Current.level, other, invalid]
  end
end
"#;

fn probe() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_framework_features_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/probe.rb", "class Probe < ApplicationRecord\nend\n")
}

fn run(mode: &str, dir: &Path, gem: Option<&Path>) -> String {
    let mut cmd = emit_and_run::ruby();
    cmd.arg(root().join("tests/framework_feature_requires.rb")).arg("run").arg(mode).arg(dir);
    if let Some(gem) = gem {
        cmd.arg(gem);
    }
    let out = cmd.output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{mode} failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_gems_rails_feature_requires_answer_from_the_ports_as_natively_from_the_loaded_frameworks() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let scratch = tree.parent().expect("scratch").to_path_buf();
    let app = scratch.join("app");
    let gem = scratch.join("probe_gem/lib");
    std::fs::create_dir_all(&gem).unwrap();
    std::fs::write(gem.join("probe_gem.rb"), PROBE_GEM).unwrap();
    let native = run("native", &app, Some(&gem));
    let emitted = run("emitted", &tree, Some(&gem));
    assert_eq!(emitted, native, "the tree's ports answered the gem differently from Rails");
    // Not vacuous: the gem ran and read each framework.
    for expected in [
        "frameworks: [\"ActiveRecord::Base\", \"ActionController::Base\", \"ActionMailer::Base\", \"ActiveJob::Base\", \"ActiveModel::Type::Value\"]",
        "tidy_bytes: \"café\"",
        "compose: [233]",
        "decompose_compatibility: [102, 105]",
        "transliterate: \"cafe\"",
        // String#parameterize through the Inflector (not the ASCII overlay).
        "parameterize: \"aeroskobing-strasse\"",
        "app_generators: [\"Rails::Configuration::Generators\", {template_engine: :tailwindcss, stylesheet_engine: :sass}, {fixture: false}, {f: :fixture}, :tailwindcss]",
        // The route block recorded, not run; isolate_namespace's methods.
        "engine: [\"probe_gem_dashboard\", true, \"probe_gem_dashboard\", [true, false, false, false, false], true, false, []]",

        // ActiveSupport::Concern: dependencies first, ClassMethods,
        // `included`/`prepended` blocks, a second `included` refused.
        "concern: [[\"ProbeGem::Record\", \"ProbeGem::SoftDeletable\", \"ProbeGem::Stamped\"], true, :record_deleted, [:deleted_at], :s, [\"ProbeGem::SoftDeletable\", \"ProbeGem::Stamped\", \"ProbeGem::PrependedRecord\"], :prependedrecord_deleted, true, \"Cannot define multiple 'included' blocks for a Concern\"]",
        "try: [nil, 3, nil, \"ABC\", nil, :nope]",
        "numeric: [12288, 1048576, 2684354560.0, 3, 1125899906842624, 172800, 5400, 1814400, 1209600, 90, 120, true, \"ActiveSupport::Duration\", 5000, 86400, true, \"1,234,567\", \"1234567.89\", \"0.500%\", \"$1,234.50\", \"1.23 Million\", \"12 KB\", \"5551234\", \"42\", \"ff\", \"3\", \"1.500\", \"1.5\", \"$1.50\", \"-1,234.50 EUR\", \"1.5 Million\", \"1 mm\", \"12.345.678.05\", \"110\", \"1.15 GB\", \"+1-123-555-1234 x 55\", \"302.24399%\", \"$abc\", nil, \"Infinity\"]",
        "class_ext: [:base, :child, :child, :base, :instance, :child, true, true, false, [], true, false, false, false, \"1 is not a symbol nor a string\", :s, [\"ProbeGem::AttrChild\", \"ProbeGem::AttrGrandChild\"], [\"ProbeGem::AttrChild\"], [], 10, true, 20, :private, 2, [false, false, false]]",
        "mattr: [:warn, :warn, [:block], [:block], :io, false, true, false, 2, 2, 5, \"invalid attribute name: bad-name\", \"module attributes should be defined directly on class, not singleton\", \"probe_gem.rb\", \"probe_gem.rb\"]",
        "strip: [\"This command does such and such.\\n\\nSupported options are:\\n  -h         This message\\n\", \"x\\n  y\"",
        "reporting: [[nil, true, false], true, [\"inside\", true], nil, \"kept\", :value, true]",
        "hash_keys: [{\"a\" => {b: [{c: 1}]}, \"d\" => 2, \"3\" => :x}",
        "Unknown key: :z. Valid keys are: :a, :b",
        "load_hooks: [[:ran, :base]]",
        "logger: [true, \"hello\\nlazy\\nsilenced\\nkept\\n\"]",
        "notifications: [:done, [[\"probe_gem.ran\", {id: 7}]]]",
        "controller_errors: [[\"ActionController::BadRequest\", \"ActionController::ActionControllerError\"]",
        "\"Only get, post, and put requests are allowed.\"",
        // Per thread, with the default seeded in each thread.
        "per_thread: [\"main\", \"main\", :info, [nil, :info, \"t\", \"t\"], \"invalid attribute name: bad-name\"]",
    ] {
        assert!(native.contains(expected), "missing {expected:?}:\n{native}");
    }
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_rails_feature_the_tree_does_not_port_is_a_named_load_error() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let out = run("refusals", &tree, None);
    for feature in ["active_support/all", "active_support/core_ext", "active_support/core_ext/hash"] {
        let line = format!("{feature}: loaded\n");
        assert!(out.contains(&line), "missing {line:?}:\n{out}");
    }
    // Every other Rails feature with no port (Integer#months, the whole
    // core_ext set): by name, not as a plain
    // `cannot load such file`, which is left to everything else.
    for feature in ["active_support/core_ext/string/filters", "action_dispatch/testing/integration", "rails/no_such_feature.rb"] {
        let line = format!(
            "{feature}: LoadError: roundhouse: error[framework_feature_not_ported]: `require {feature:?}`: this tree has no port of this Rails feature (project.rs::FRAMEWORK_FEATURE_PORTS), and never loads the gem home's Rails; not loaded\n"
        );
        assert!(out.contains(&line), "missing {line:?}:\n{out}");
    }
    for line in [
        "active_model_serializers_probe: LoadError: cannot load such file -- active_model_serializers_probe\n",
        "probe_missing/active_support: LoadError: cannot load such file -- probe_missing/active_support\n",
        // A framework feature's file that loads but itself requires what
        // is missing keeps the inner LoadError.
        "active_support/probe_inner: LoadError: cannot load such file -- probe_gem_inner_dependency\n",
    ] {
        assert!(out.contains(line), "missing {line:?}:\n{out}");
    }
    assert_eq!(out.matches("framework_feature_not_ported").count(), 3, "{out}");
    assert!(out.contains("routes: NotImplementedError: error[route_set_not_drawn]: the routes block at "), "{out}");
    for method in ["url_helpers", "recognize_path"] {
        let line = format!("routes: NotImplementedError: error[route_set_not_drawn]: ActionDispatch::Routing::RouteSet#{method} needs a drawn route set");
        assert!(out.contains(&line), "missing {line:?}:\n{out}");
    }
    assert_eq!(out.matches("RouteSet#url_helpers needs a drawn route set").count(), 2, "{out}");
    assert_eq!(out.matches("routes: ").count(), 4, "{out}");
    let features = tree.join("runtime/features");
    let read = |feature: &str| std::fs::read_to_string(features.join(format!("{feature}.rb"))).unwrap_or_else(|e| panic!("{feature}: {e}"));
    assert!(read("active_job").contains("require_relative \"../active_job\"\n"), "{}", read("active_job"));
    assert!(read("active_support/multibyte/unicode").contains("require_relative \"../../../active_support_multibyte_unicode\"\n"));
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(facades.contains("require_relative \"active_support_inflector\"\nrequire_relative \"active_support_multibyte_unicode\"\n"), "{facades}");
}

/// Numeric#to_fs and the I18n-backed number helpers translate through I18n:
/// with no i18n in the lock the tree carries neither them nor core_ext/numeric
/// (all three Numeric extensions), which the refusal then names, while the
/// byte and Duration builders still answer their own features.
#[test]
#[ignore = "fork spec: postgresql"]
fn core_ext_numeric_needs_i18n_for_its_number_helpers() {
    let lock = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n\nDEPENDENCIES\n  concurrent-ruby\n";
    let (tree, errors) = probe()
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\n")
        .write("Gemfile.lock", lock)
        .emit(BuildTarget::Ruby);
    // The probe's own diagnostics (its postgresql declaration), no more:
    // dropping i18n adds none.
    let (_, base) = probe().emit(BuildTarget::Ruby);
    assert_eq!(format!("{errors:?}"), format!("{base:?}"));
    let features = tree.join("runtime/features/active_support/core_ext");
    for absent in ["numeric.rb", "numeric/conversions.rb"] {
        assert!(!features.join(absent).exists(), "{absent} without i18n");
    }
    // The shared Ruby-family NumberHelper (runtime/ruby, translation tables of
    // its own) stays: action_view_number_helper.rb and active_support_ext.rb
    // require it whatever the lock carries. What i18n adds is the port that
    // translates through I18n, which this tree must not carry.
    let helper = std::fs::read_to_string(tree.join("runtime/active_support_number_helper.rb")).expect("number helper");
    assert!(helper.contains("A finite translation backend"), "{helper}");
    assert!(!helper.contains("active_support_i18n"), "the I18n-backed number helper without i18n");
    for present in ["numeric/bytes.rb", "numeric/time.rb", "class.rb", "string/strip.rb", "module/attribute_accessors.rb"] {
        assert!(features.join(present).exists(), "{present}");
    }
}
