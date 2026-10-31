//! A gem compiled as source boots in the order its own Ruby loads:
//! autoloaded nested classes, aliases of inherited and core methods, and
//! declaration-only files a boot requires.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn native(files: &[(&str, &str)], script: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("rh_boot_emit_order_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = emit_and_run::ruby().arg("-I").arg(root.join("lib")).arg("-e").arg(script).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const ID_SERVICE: &str = "require \"socket\"\n\nmodule IdService\n  class Error < StandardError; end\n\n  SCHEMA_NAME = \"main\"\n\n  module Validation\n    MAX_COUNT = 65535\n\n    def check(n) = n <= MAX_COUNT\n  end\n\n  class Local\n    extend(Validation)\n\n    def self.ok = check(3)\n  end\nend\n";

#[test]
fn a_module_declared_above_a_class_loads_before_it() {
    let script = "p IdService::Local.ok\np IdService::Error.superclass\n";
    let expected = native(&[("lib/id_service.rb", ID_SERVICE)], &format!("require \"id_service\"\n{script}"));
    assert_eq!(expected, "true\nStandardError\n");
    let run = emit_and_run::empty_app().write("lib/id_service.rb", ID_SERVICE).run_ruby(script);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

const COMPONENT_CONFIG: &str = "module Component\n  class Config\n    class << self\n      def inherited(config)\n        return super unless config.superclass == Config\n\n        if (class_name = config.name) &&\n            (config_location = Object.const_source_location(class_name)) &&\n            (parent_dirname = File.dirname(config_location[0]))\n          config_dirname = File.join(parent_dirname, \"config\")\n          config.autoload(:Test, File.join(config_dirname, \"test.rb\"))\n        end\n\n        super\n      end\n\n      def label = \"workflows\"\n\n      def for(env)\n        const_get(env.capitalize)\n      end\n    end\n  end\nend\n";
const WORKFLOWS_CONFIG: &str = "require \"component/config\"\n\nmodule WidgetFlowsComponent\n  class Config < ::Component::Config\n  end\nend\n";
const WORKFLOWS_CONFIG_TEST: &str = "module WidgetFlowsComponent\n  class Config\n    class Test < Config\n    end\n  end\nend\n";

#[test]
#[ignore = "fork spec: load order"]
fn a_nested_class_written_in_its_own_file_keeps_the_file_its_autoload_names() {
    let files = [
        ("lib/component/config.rb", COMPONENT_CONFIG),
        ("lib/widget_flows_component/config.rb", WORKFLOWS_CONFIG),
        ("lib/widget_flows_component/config/test.rb", WORKFLOWS_CONFIG_TEST),
    ];
    // The emitted boot loads eagerly, which resolves the autoload; one
    // still pending names a file that exists, lazily (native) or not.
    let script = "c = WidgetFlowsComponent::Config\npending = c.autoload?(:Test)\np pending.nil? || File.exist?(pending)\nt = c.for(\"test\")\np t, t.superclass, t.label\n";
    let expected = native(&files, &format!("require \"widget_flows_component/config\"\np File.basename(WidgetFlowsComponent::Config.autoload?(:Test))\n{script}"));
    assert_eq!(expected, "\"test.rb\"\ntrue\nWidgetFlowsComponent::Config::Test\nWidgetFlowsComponent::Config\n\"workflows\"\n");
    let expected = expected.strip_prefix("\"test.rb\"\n").unwrap();
    let mut app = emit_and_run::empty_app();
    for (path, text) in files {
        app = app.write(path, text);
    }
    let run = app.run_ruby(script);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

/// monorepo_slices' slices_file_contract.rb: the contract's constants,
/// then its validator classes, one of which reads a constant above it
/// as its body loads (`/\A#{SLICE_LABEL_PREFIX}.+\z/`), then a constant
/// built from a validator. Each nested class was spliced in ahead of
/// all of its parent's constants, so the read raised NameError at boot.
const SLICES_FILE_CONTRACT: &str = "module MonorepoSlices\n  class SlicesFileContract\n    MAX_GITHUB_LABEL_LENGTH = 50\n    SLICE_LABEL_PREFIX = \"Slice: \"\n    SLICE_LABEL_LENGTH = SLICE_LABEL_PREFIX.length\n\n    def self.validators = [DEFAULT_VALIDATOR]\n\n    class BaseValidator\n      def ok?(label) = label.length <= MAX_GITHUB_LABEL_LENGTH\n    end\n\n    class IssueTrackingLabelValidator < BaseValidator\n      ISSUE_LABEL_REGEX = /\\A#{SLICE_LABEL_PREFIX}.+\\z/\n\n      def ok?(label) = super && ISSUE_LABEL_REGEX.match?(label)\n    end\n\n    DEFAULT_VALIDATOR = IssueTrackingLabelValidator.new\n  end\nend\n";

#[test]
fn a_nested_class_loads_after_the_parent_constants_above_it() {
    let files = [("lib/monorepo_slices/slices_file_contract.rb", SLICES_FILE_CONTRACT)];
    let script = "c = MonorepoSlices::SlicesFileContract\np c::SLICE_LABEL_LENGTH, c.validators.map { |v| [v.class, v.ok?(\"Slice: x\"), v.ok?(\"x\")] }, c.constants.sort\n";
    let expected = native(&files, &format!("require \"monorepo_slices/slices_file_contract\"\n{script}"));
    assert_eq!(expected, "7\n[[MonorepoSlices::SlicesFileContract::IssueTrackingLabelValidator, true, false]]\n[:BaseValidator, :DEFAULT_VALIDATOR, :IssueTrackingLabelValidator, :MAX_GITHUB_LABEL_LENGTH, :SLICE_LABEL_LENGTH, :SLICE_LABEL_PREFIX]\n");
    let run = emit_and_run::empty_app().write(files[0].0, files[0].1).run_ruby(script);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

fn gem_same_as_native(name: &str, files: &[(&str, &str)], entry: &str, script: &str, expected: &str) -> emit_and_run::Run {
    let lib: Vec<(String, &str)> = files.iter().map(|(path, text)| (format!("lib/{path}"), *text)).collect();
    let lib: Vec<(&str, &str)> = lib.iter().map(|(path, text)| (path.as_str(), *text)).collect();
    assert_eq!(native(&lib, &format!("require {entry:?}\n{script}")), expected, "native control");
    let mut overlay = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", &format!("PATH\n  remote: vendor/gems/{name}-1.0.0\n  specs:\n    {name} (1.0.0)\n\nDEPENDENCIES\n  {name}!\n"))
        .write("config/application.rb", &format!("require {entry:?}\n"));
    for (path, text) in files {
        overlay = overlay.write(&format!("vendor/gems/{name}-1.0.0/lib/{path}"), text);
    }
    let run = overlay.run_ruby(script);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert!(!run.stderr.contains("file_require_not_carried"), "{}", run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
    run
}

/// graphql-client 0.25.0: collocated_enforcement.rb reopens `Client`,
/// writes `WHITELISTED_GEM_NAMES` and then `module CollocatedEnforcement`;
/// client.rb requires it and then runs `extend CollocatedEnforcement` in
/// `Client`'s body. The module goes into Client's file below that
/// constant, and it was written after all of Client's constants, so after
/// client.rb's `extend`, which raised NameError at boot.
const COLLOCATED: &[(&str, &str)] = &[
    ("gqlc/error.rb", "module Gqlc\n  class Client\n    class Error < StandardError; end\n  end\nend\n"),
    (
        "gqlc/collocated.rb",
        "require \"gqlc/error\"\n\nmodule Gqlc\n  class Client\n    NAMES = %w{pry byebug}\n\n    class CallerError < Error; end\n\n    module Collocated\n      def collocated = [:collocated, NAMES.size]\n    end\n  end\nend\n",
    ),
    (
        "gqlc/client.rb",
        "require \"gqlc/collocated\"\n\nmodule Gqlc\n  class Client\n    class DynamicError < Error; end\n\n    extend Collocated\n\n    DOC = collocated\n  end\nend\n",
    ),
];

#[test]
#[ignore = "fork spec: load order"]
fn a_nested_module_from_another_file_loads_before_the_body_that_extends_it() {
    gem_same_as_native(
        "gqlc",
        COLLOCATED,
        "gqlc/client",
        "c = Gqlc::Client\np c::DOC, c.collocated, c::CallerError.superclass, c::DynamicError.superclass, c.constants.sort\n",
        "[:collocated, 2]\n[:collocated, 2]\nGqlc::Client::Error\nGqlc::Client::Error\n[:CallerError, :Collocated, :DOC, :DynamicError, :Error, :NAMES]\n",
    );
}

/// graphql-client 0.25.0's operation_definition.rb:
/// `alias operation_name definition_name` in `OperationDefinition <
/// Definition` names the superclass's method. The ingest refused the
/// file, so graphql/client.rb's `require "graphql/client/operation_definition"`
/// named a file the tree did not carry (file_require_not_carried at boot).
const GQLC: &[(&str, &str)] = &[
    ("gqlc.rb", "require \"gqlc/operation_definition\"\n\nmodule Gqlc\n  def self.name_of(definition) = definition.operation_name\nend\n"),
    ("gqlc/definition.rb", "module Gqlc\n  class Definition\n    def initialize(name)\n      @name = name\n    end\n\n    def definition_name = @name\n\n    class << self\n      def build(name) = new(name)\n    end\n  end\nend\n"),
    (
        "gqlc/operation_definition.rb",
        "require \"gqlc/definition\"\n\nmodule Gqlc\n  class OperationDefinition < Definition\n    # Public: Alias for definition name.\n    alias operation_name definition_name\n\n    class << self\n      alias create build\n    end\n  end\nend\n",
    ),
];

#[test]
fn an_alias_of_an_inherited_method_is_carried() {
    gem_same_as_native(
        "gqlc",
        GQLC,
        "gqlc",
        "d = Gqlc::OperationDefinition.new(\"Q\")\np d.operation_name, Gqlc.name_of(d), Gqlc::OperationDefinition.create(\"R\").operation_name\np Gqlc::OperationDefinition.instance_method(:operation_name).original_name\n",
        "\"Q\"\n\"Q\"\n\"R\"\n:definition_name\n",
    );
}

const CORE_ALIAS: &[(&str, &str)] = &[
    ("amlike.rb", "require \"amlike/errors\"\n"),
    (
        "amlike/errors.rb",
        "module Amlike\n  class Errors < Hash\n    def initialize\n      super() { |h, k| h[k] = [] }\n    end\n\n    alias count size\n\n    def [](key)\n      super(key.to_s)\n    end\n  end\n\n  class Plainer\n    alias described inspect\n  end\nend\n",
    ),
];

#[test]
#[ignore = "fork spec: core method signatures"]
fn an_alias_of_a_core_superclass_method_is_carried() {
    gem_same_as_native(
        "amlike",
        CORE_ALIAS,
        "amlike",
        "e = Amlike::Errors.new\ne[:card] << \"bad\"\ne[\"name\"] << \"blank\"\np e.count, e[\"card\"], Amlike::Errors.instance_method(:count).original_name\np Amlike::Plainer.new.described.start_with?(\"#<Amlike::Plainer\")\n",
        "2\n[\"bad\"]\n:size\ntrue\n",
    );
}

/// graphql-client 0.25.0's query_typename.rb declares
/// `InsertTypenameVisitor` in `module QueryTypename` only in the branch
/// of `if GraphQL::Language::Nodes::AbstractNode.method_defined?(:merge)`
/// that finds graphql-ruby's node mutation API. The conditional is now
/// carried with the class inside its arm (tests/conditional_class_declarations.rs
/// runs both branches), so the file is no longer refused. A class
/// declared in a conditional inside `class << self` is still named as
/// the gap.
const QUERY_TYPENAME: &str = "module Gqlc\n  module QueryTypename\n    if ENV[\"GQLC_LEGACY\"].nil?\n      class InsertVisitor\n        def visit = :visitor\n      end\n\n      def self.insert = InsertVisitor.new.visit\n    else\n      def self.insert = :legacy\n    end\n\n    def self.flatten = 1\n  end\nend\n";

const SINGLETON_CONDITIONAL: &str = "module Gqlc\n  module Singleton\n    def self.x = 1\n\n    class << self\n      if ENV[\"GQLC_LEGACY\"].nil?\n        class Inner\n          def visit = :visitor\n        end\n      end\n    end\n  end\nend\n";

#[test]
fn a_class_declared_in_a_class_body_conditional_is_carried() {
    let files = [("lib/gqlc/query_typename.rb", QUERY_TYPENAME)];
    let script = "require \"gqlc/query_typename\"\np Gqlc::QueryTypename.insert, defined?(Gqlc::QueryTypename::InsertVisitor)\n";
    assert_eq!(native(&files, script), ":visitor\n\"constant\"\n");
    assert_eq!(native(&files, &format!("ENV[\"GQLC_LEGACY\"] = \"1\"\n{script}")), ":legacy\nnil\n");

    let text = check_gem_tree("carried", &files);
    assert!(!text.contains("query_typename.rb"), "{text}");

    let text = check_gem_tree("singleton", &[("lib/gqlc/singleton.rb", SINGLETON_CONDITIONAL)]);
    let row = text.lines().find(|l| l.contains("singleton.rb:7:9:")).unwrap_or_else(|| panic!("{text}"));
    assert!(
        row.contains("a class or module declared in a load-time conditional inside `class << self` is not modeled"),
        "{row}"
    );
}

/// `roundhouse check` on a tree vendoring the gem `gqlc` with `files`
/// (paths in the gem): its rows.
fn check_gem_tree(tag: &str, files: &[(&str, &str)]) -> String {
    let root = std::env::temp_dir().join(format!("rh_boot_emit_order_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let gem: Vec<(String, &str)> = files.iter().map(|(path, text)| (format!("vendor/gems/gqlc-1.0.0/{path}"), *text)).collect();
    for (path, text) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("roundhouse.yml", ""),
        ("Gemfile.lock", "PATH\n  remote: vendor/gems/gqlc-1.0.0\n  specs:\n    gqlc (1.0.0)\n\nDEPENDENCIES\n  gqlc!\n"),
    ]
    .into_iter()
    .chain(gem.iter().map(|(path, text)| (path.as_str(), *text)))
    {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const UNDEFINED_ALIAS: &[(&str, &str)] = &[
    ("lib/gqlc/base.rb", "module Gqlc\n  class Base\n    def name = \"base\"\n  end\nend\n"),
    ("lib/gqlc/node.rb", "require \"gqlc/base\"\n\nmodule Gqlc\n  class Node < Base\n    alias label gem_label\n  end\nend\n"),
];

/// An `alias` whose original no ancestor in the tree defines is not
/// captured: native Ruby raises loading the file, and the file is still
/// refused at the alias, as before.
#[test]
fn an_alias_no_ancestor_defines_still_refuses_its_file() {
    let script = "begin\n  require \"gqlc/node\"\nrescue NameError => e\n  p e.name\nend\n";
    assert_eq!(native(UNDEFINED_ALIAS, script), ":gem_label\n");
    let text = check_gem_tree("undefined_alias", UNDEFINED_ALIAS);
    let row = text.lines().find(|l| l.contains("node.rb:5:5:")).unwrap_or_else(|| panic!("{text}"));
    assert!(row.contains("alias names a method this body has not defined"), "{row}");
}

/// A gem file of declarations only, required by config/application.rb.
/// The boot list held file programs only, so a file with none loaded
/// nothing, and a plain gem's constants have no autoload: `Gqlc::Plain`
/// was undefined (NameError). Each class it declares now loads at that
/// boot require, in the file's order (`Shapes` before `Plain`, whose
/// body reads it).
const PLAIN_DECLARATIONS: &[(&str, &str)] = &[(
    "gqlc/plain.rb",
    "module Gqlc\n  module Shapes\n    ALL = %i[a b c].freeze\n  end\n\n  class Plain\n    SIZE = Shapes::ALL.size\n\n    def self.x = SIZE\n  end\nend\n",
)];

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_file_of_declarations_only_required_by_boot_is_loaded() {
    let run = gem_same_as_native(
        "gqlc",
        PLAIN_DECLARATIONS,
        "gqlc/plain",
        "p Gqlc::Plain.x, Gqlc::Shapes::ALL.last\n",
        "3\n:c\n",
    );
    let models = std::fs::read_to_string(run.emitted.join("app/models.rb")).unwrap();
    let shapes = models.find("  [\"models/gqlc/shapes\", nil], # config/application.rb:1\n");
    let plain = models.find("  [\"models/gqlc/plain\", nil], # config/application.rb:1\n");
    assert!(shapes.is_some() && plain.is_some() && shapes < plain, "{models}");
}

const SINGLETON_ONLY: &[(&str, &str)] = &[(
    "gqlc/singleton.rb",
    "module Gqlc\n  module Singleton\n    class << self\n      class Inner\n        def visit = :visitor\n      end\n\n      def make = Inner.new.visit\n    end\n  end\nend\n",
)];

const SINGLETON_BESIDE_DEF: &[(&str, &str)] = &[(
    "gqlc/singleton.rb",
    "module Gqlc\n  module Singleton\n    def self.x = 1\n\n    class << self\n      class Inner\n        def visit = :visitor\n      end\n\n      def make = [Inner.new.visit, x]\n    end\n  end\nend\n",
)];

#[test]
fn a_class_declared_in_class_less_less_self_of_a_gem_module_is_loaded() {
    gem_same_as_native("gqlc", SINGLETON_ONLY, "gqlc/singleton", "p Gqlc::Singleton.make\n", ":visitor\n");
    gem_same_as_native("gqlc", SINGLETON_BESIDE_DEF, "gqlc/singleton", "p Gqlc::Singleton.make, Gqlc::Singleton.x\n", "[:visitor, 1]\n1\n");
}

/// activemerchant 1.137.0's `CreditCard`: a class-side `attr_accessor`
/// in `class << self` that the body then writes (`self.require_name =
/// true`), and writers made by a loop over a literal list that
/// `class_eval`s one def per element. The write was dropped as an
/// unmodelled class-body call, and the loop with no diagnostic, so
/// `Model#initialize`'s `send("month=", ...)` raised NoMethodError.
const CARDLIKE: &[(&str, &str)] = &[(
    "cardlike.rb",
    r##"module Cardlike
  class Card
    class << self
      attr_accessor :require_name
    end

    self.require_name = true

    %w(month year).each do |m|
      class_eval <<~RUBY, __FILE__, __LINE__ + 1
        def #{m}=(v)
          @#{m} = case v
          when "", nil, 0
            nil
          else
            v.to_i
          end
        end
      RUBY
    end

    attr_reader :month, :year

    def requires_name? = self.class.require_name
  end

  class Gift < Card
    self.require_name = false
  end
end
"##,
)];

#[test]
fn a_class_side_writer_call_and_a_class_eval_loop_run_as_native() {
    gem_same_as_native(
        "cardlike",
        CARDLIKE,
        "cardlike",
        "c = Cardlike::Card.new\nc.month = \"9\"\nc.year = \"\"\np [c.month, c.year, c.requires_name?, Cardlike::Gift.new.requires_name?, Cardlike::Card.public_method_defined?(:month=)]\np [Cardlike::Card.require_name, Cardlike::Gift.require_name]\n",
        "[9, nil, true, false, true]\n[true, false]\n",
    );
}

/// activemerchant 1.137.0's billing/compatibility.rb ends `module
/// Billing` with `Compatibility::Model.include Rails::Model`: one app
/// module including another from a third body, which gives
/// `Compatibility::Model#valid?`'s `super` its method. It was dropped as
/// an unmodelled class-body call (NoMethodError: no superclass method).
const COMPAT: &[(&str, &str)] = &[(
    "compat.rb",
    r##"module Compat
  module Billing
    module Compatibility
      module Model
        def valid?
          [:compat, super]
        end
      end
    end

    module Rails
      module Model
        def valid? = :rails
      end
    end

    class Card
      include Compatibility::Model
    end

    Compatibility::Model.include Rails::Model
  end
end
"##,
)];

#[test]
fn an_include_written_in_another_body_is_the_modules_own() {
    gem_same_as_native(
        "compat",
        COMPAT,
        "compat",
        "b = Compat::Billing\np b::Card.new.valid?, b::Compatibility::Model.ancestors, b::Card.ancestors.take(3)\n",
        "[:compat, :rails]\n[Compat::Billing::Compatibility::Model, Compat::Billing::Rails::Model]\n[Compat::Billing::Card, Compat::Billing::Compatibility::Model, Compat::Billing::Rails::Model]\n",
    );
}

/// A class-body `self.x = v` in a gem source whose writer `x=` an
/// included module's `self.included(base)` hook defines at run time, in
/// a hook the ingest keeps as text (its names are computed, so
/// `hook_class_attribute` does not expand it): no app method answers
/// `x=` statically. The write was dropped as an unmodelled class-body
/// call, so the class read the hook's default.
const HOOKED_WRITER: &[(&str, &str)] = &[(
    "hooked.rb",
    r##"module Hooked
  module Settings
    NAMES = %i[rate limit].freeze

    def self.included(base)
      base.singleton_class.send(:attr_accessor, *NAMES)
      NAMES.each { |n| base.public_send("#{n}=", :default) }
    end
  end

  module Flags
    def self.included(base)
      %w[strict].each do |n|
        base.singleton_class.class_eval { attr_accessor n }
      end
    end
  end

  class Gateway
    include Settings

    self.rate = 3
  end

  class Child < Gateway
    include Flags

    self.limit = 9
    self.strict ||= :on
  end
end
"##,
)];

#[test]
fn a_class_body_write_through_a_writer_an_included_hook_defines_runs_as_native() {
    gem_same_as_native(
        "hooked",
        HOOKED_WRITER,
        "hooked",
        "h = Hooked\np [h::Gateway.rate, h::Gateway.limit, h::Child.rate, h::Child.limit, h::Child.strict]\n",
        "[3, :default, nil, 9, :on]\n",
    );
}

/// activeresource's http_mock.rb writes `HttpMock`'s verbs in a loop over
/// a literal Hash of `%w[]` lists, its heredoc interpolating a
/// conditional on the key. The statement was dropped as a call on a
/// computed receiver, so `mock.post(...)` raised NoMethodError and no
/// HttpMock connection could send a request.
const VERBS: &[(&str, &str)] = &[(
    "verbs.rb",
    r##"module Verbs
  class Mock
    { true  => %w[post put],
      false => %w[get head] }.each do |has_body, methods|
      methods.each do |method|
        module_eval <<-EOE, __FILE__, __LINE__ + 1
          def #{method}(path, #{'body, ' if has_body}headers, options = {})
            [:#{method}, path, #{has_body ? 'body, ' : 'nil, '}headers, options]
          end
        EOE
      end
    end
  end
end
"##,
)];

#[test]
fn a_module_eval_loop_over_a_literal_hash_of_lists_runs_as_native() {
    gem_same_as_native(
        "verbs",
        VERBS,
        "verbs",
        "m = Verbs::Mock.new\np m.post(\"/a\", \"b\", {})\np m.get(\"/c\", {}, 1)\np Verbs::Mock.instance_method(:put).arity, Verbs::Mock.instance_method(:head).arity\n",
        "[:post, \"/a\", \"b\", {}, {}]\n[:get, \"/c\", nil, {}, 1]\n-4\n-3\n",
    );
}

/// carriers_db's `Carrier`: `add_country_index` is a class method of the
/// class that the body calls right below its def, to register an index in
/// a class-side registry. The call was dropped as an unmodelled class-body
/// call, so the registry stayed empty.
const OWN_CLASS_CALL: &[(&str, &str)] = &[(
    "indexed.rb",
    r##"module Indexed
  class Carrier
    class << self
      attr_accessor :index_definitions
    end

    self.index_definitions = {}.freeze

    def self.add_country_index
      self.index_definitions = index_definitions.merge("country" => :country).freeze
    end

    add_country_index
  end
end
"##,
)];

#[test]
fn a_bare_call_to_the_classs_own_class_method_runs_after_its_def() {
    gem_same_as_native(
        "indexed",
        OWN_CLASS_CALL,
        "indexed",
        "p Indexed::Carrier.index_definitions\n",
        "{\"country\" => :country}\n",
    );
}
