//! A gem compiled as source loads as its Ruby does: data files its code
//! reads, module-body order, compact namespace headers, and requires
//! written below a class.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LOCK: &str = "PATH\n  remote: vendor/gems/tessera-1.2.3\n  specs:\n    tessera (1.2.3)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  tessera!\n";

const FILES: &[(&str, &str)] = &[
    ("VERSION", "1.2.3\n"),
    (
        "lib/tessera.rb",
        "require 'logger'\n\nmodule Tessera\n  VERSION = ::File.read(\n    ::File.join(::File.dirname(__FILE__), '../VERSION')\n  ).strip\n\n  def self.logger\n    @@logger\n  end\n  def self.logger=(logger)\n    @@logger = logger\n  end\n  self.logger = ::Logger.new(nil)\n  self.logger.progname = 'Tessera'\n\n  def self.debugging?\n    @@debugging\n  end\n  def self.debugging=(boolean)\n    @@debugging = boolean\n  end\n  self.debugging = false\n\n  def self.describe\n    \"#{VERSION} #{logger.progname} #{debugging?}\"\n  end\nend\n\nrequire 'tessera/wire'\nrequire 'tessera/wire/frames'\nrequire 'tessera/json/leaf_version'\n",
    ),
    ("lib/tessera/wire.rb", "module Tessera\n  module Wire\n    module Codec\n    end\n  end\nend\n"),
    (
        "lib/tessera/wire/frames.rb",
        "module Tessera::Wire::Codec::Frames\n  def self.tag\n    :frames\n  end\nend\n",
    ),
    (
        "lib/tessera/json/leaf_version.rb",
        "module Tessera\n  module Json\n    LEAF_VERSION = ::File.read(::File.join(::File.dirname(__FILE__), '../../../VERSION')).strip\n  end\nend\n",
    ),
];

const OBSERVE: &str = r##"
puts Tessera.describe
puts Tessera::Json::LEAF_VERSION
puts Tessera::Wire::Codec::Frames.tag.inspect
"##;

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn write_gem(root: &std::path::Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

fn app_with(files: &[(&str, &str)], application: &str) -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/application.rb", application);
    for (path, text) in files {
        app = app.write(&format!("vendor/gems/tessera-1.2.3/{path}"), text);
    }
    app
}

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_source_loads_its_data_modules_and_requires_like_the_gem() {
    let gem = std::env::temp_dir().join(format!("rh_tessera_native_{}", std::process::id()));
    write_gem(&gem, FILES);
    let native = emit_and_run::ruby().arg("-I").arg(gem.join("lib")).args(["-rtessera", "-e", OBSERVE]).output().expect("CRuby control");
    assert!(native.status.success(), "native tessera: {}", String::from_utf8_lossy(&native.stderr));
    let expected = lines(&String::from_utf8_lossy(&native.stdout));
    std::fs::remove_dir_all(&gem).unwrap();
    assert_eq!(expected, ["1.2.3 Tessera false", "1.2.3", ":frames"], "native control");

    let run = app_with(FILES, "require \"tessera\"\n").run_ruby(OBSERVE);
    assert!(
        run.success,
        "the emitted program failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}\nerrors: {:#?}",
        run.emitted.display(),
        run.stdout,
        run.stderr,
        run.errors
    );
    assert_eq!(lines(&run.stdout), expected, "emitted Tessera diverges from the gem");
}

/// A gem file whose only classes extend a Rails base the runtime does not
/// port (a generator: `Rails::Generators::Base`) is dropped with a
/// `lower_residue` warning, and no file of it is written; the `require`
/// that names it has nothing to load, as natively it loads a file whose
/// classes are never used at run time.
#[test]
fn requiring_a_gem_file_whose_classes_were_all_dropped_loads_nothing() {
    let files: &[(&str, &str)] = &[
        (
            "lib/tessera.rb",
            "module Tessera\n  def self.describe\n    :ok\n  end\nend\n\nrequire 'generators/tessera'\n",
        ),
        (
            "lib/generators/tessera.rb",
            "require_relative 'tessera/scaffold_generator'\n\nmodule Tessera\nend\n",
        ),
        (
            "lib/generators/tessera/scaffold_generator.rb",
            "module Tessera\n  class ScaffoldGenerator < Rails::Generators::Base\n    def go\n      :go\n    end\n  end\nend\n",
        ),
    ];
    let run = app_with(files, "require \"tessera\"\n").run_ruby("puts Tessera.describe\n");
    assert!(
        run.success,
        "the emitted program failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}\nerrors: {:#?}",
        run.emitted.display(),
        run.stdout,
        run.stderr,
        run.errors
    );
    assert_eq!(lines(&run.stdout), ["ok"]);
}

/// A class that is an ActiveModel class (`include ActiveModel::Validations`
/// with a `validate`) is carried as a model, whose file has no file
/// program. A `require` written below it, of a subclass file that relies
/// on the class being loaded (it names the class without requiring it,
/// as openid_connect's `request_object.rb` does `connect_object`), runs
/// after the class is defined, as in the source: hoisted above the class,
/// the subclass file loaded first and `Node` was an uninitialized
/// constant.
#[test]
#[ignore = "fork spec: load order"]
fn a_require_written_below_a_gem_class_runs_after_the_class_is_defined() {
    let files: &[(&str, &str)] = &[
        ("lib/tessera.rb", "require 'tessera/node'\n"),
        (
            "lib/tessera/node.rb",
            "module Tessera\n  class Node\n    include ActiveModel::Validations\n\n    validate :check\n\n    def check\n    end\n\n    def kind\n      :node\n    end\n  end\nend\n\nrequire 'tessera/leaf'\n",
        ),
        (
            "lib/tessera/leaf.rb",
            "module Tessera\n  class Leaf < Node\n    def kind\n      :leaf\n    end\n  end\nend\n\nTessera.const_set(:LEAF, Tessera::Leaf)\n",
        ),
    ];
    let run = app_with(files, "require \"tessera\"\n").run_ruby("puts \"#{Tessera::Leaf.new.kind} #{Tessera::Leaf.superclass} #{Tessera::LEAF}\"\n");
    let node = std::fs::read_to_string(run.emitted.join("app/models/tessera/node.rb")).expect("emitted node.rb");
    let class = node.find("class Node").expect("class Node emitted");
    let require = node.find("require_relative").unwrap_or_else(|| panic!("the trailing require is carried:\n{node}"));
    assert!(class < require, "the require written below the class moved above it:\n{node}");
    assert!(
        run.success,
        "the emitted program failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}\nerrors: {:#?}",
        run.emitted.display(),
        run.stdout,
        run.stderr,
        run.errors
    );
    assert_eq!(lines(&run.stdout), ["leaf Tessera::Node Tessera::Leaf"]);
}

/// A gem file that opens a namespace nested (`module Tessera; module
/// Logger`) defines it when that file loads, as money-kit 4.1.1's
/// `money/rails/job_argument_serializer.rb` (`class Money; module Rails`)
/// does. Until then a lexical `Logger` inside `module Tessera` is the top
/// level's, as `Rails::Railtie` inside `class Money` is natively; the
/// emitted tree must not define the nested namespace ahead of its file.
#[test]
#[ignore = "fork spec: load order"]
fn a_nested_namespace_a_later_file_opens_does_not_shadow_the_top_level_before_it_loads() {
    const NESTED: &[(&str, &str)] = &[
        (
            "lib/tessera.rb",
            "require 'logger'\n\nmodule Tessera\n  class Formatter < Logger::Formatter\n  end\nend\n\nrequire 'tessera/logger/sink'\n",
        ),
        (
            "lib/tessera/logger/sink.rb",
            "module Tessera\n  module Logger\n    class Sink\n      def self.tag\n        :sink\n      end\n    end\n  end\nend\n",
        ),
    ];
    const SEE: &str = "puts Tessera::Formatter.superclass.name\nputs Tessera::Logger::Sink.tag.inspect\n";
    let gem = std::env::temp_dir().join(format!("rh_tessera_nested_native_{}", std::process::id()));
    write_gem(&gem, NESTED);
    let native = emit_and_run::ruby().arg("-I").arg(gem.join("lib")).args(["-rtessera", "-e", SEE]).output().expect("CRuby control");
    assert!(native.status.success(), "native tessera: {}", String::from_utf8_lossy(&native.stderr));
    let expected = lines(&String::from_utf8_lossy(&native.stdout));
    std::fs::remove_dir_all(&gem).unwrap();
    assert_eq!(expected, ["Logger::Formatter", ":sink"], "native control");

    let run = app_with(NESTED, "require \"tessera\"\n").run_ruby(SEE);
    assert!(
        run.success,
        "the emitted program failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}\nerrors: {:#?}",
        run.emitted.display(),
        run.stdout,
        run.stderr,
        run.errors
    );
    assert_eq!(lines(&run.stdout), expected, "emitted Tessera diverges from the gem");
}

/// The `a_require_written_below_a_gem_class_runs_after_the_class_is_defined`
/// shape under `ruby -w`, where a `require` of a file that is
/// still loading prints "loading in progress, circular require considered
/// harmful" (a StrictWarning-style boot raises on it). Natively the
/// subclass file never requires its superclass's file back: the
/// superclass is defined by the time the tail require runs. The emitted
/// subclass file's own dependency require of the superclass file
/// (`require_relative "node"`) must therefore not run while that file is
/// still loading.
#[test]
#[ignore = "fork spec: load order"]
fn a_require_written_below_a_gem_class_does_not_re_enter_the_loading_file() {
    let files: &[(&str, &str)] = &[
        ("lib/tessera.rb", "require 'tessera/node'\n"),
        (
            "lib/tessera/node.rb",
            "module Tessera\n  class Node\n    include ActiveModel::Validations\n\n    validate :check\n\n    def check\n    end\n  end\nend\n\nrequire 'tessera/leaf'\nrequire 'tessera/twig'\n",
        ),
        ("lib/tessera/leaf.rb", "module Tessera\n  class Leaf < Node\n    def kind\n      :leaf\n    end\n  end\nend\n"),
        ("lib/tessera/twig.rb", "module Tessera\n  class Twig < Node\n    def kind\n      :twig\n    end\n  end\nend\n"),
    ];
    let run = app_with(files, "require \"tessera\"\n")
        .run_ruby_env("puts \"#{Tessera::Leaf.new.kind} #{Tessera::Twig.new.kind}\"\n", &[("RUBYOPT", "-w")]);
    assert!(
        run.success,
        "the emitted program failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}\nerrors: {:#?}",
        run.emitted.display(),
        run.stdout,
        run.stderr,
        run.errors
    );
    assert_eq!(lines(&run.stdout), ["leaf twig"]);
    let circular: Vec<&str> = run.stderr.lines().filter(|l| l.contains("circular require")).collect();
    assert!(circular.is_empty(), "a require re-entered a loading file: {circular:#?}");
}
