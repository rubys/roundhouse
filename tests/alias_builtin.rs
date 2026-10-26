//! `alias new old` where `old` is a method every Object answers (`send`,
//! `to_s`, `class`, …) rather than one the class defines. The
//! ActiveModel::Serializers convention is `alias
//! read_attribute_for_serialization send`. Ruby captures the method at the
//! alias, so each alias answers as the builtin does (Ruby 4.0.7).
//!
//! The alias was refused ("alias names a method this body has not
//! defined"), which left the class an unsupported constant. On the ruby
//! family it now delegates to the builtin. A class that defines the
//! builtin's name itself stays the gap, because the delegation would reach
//! that definition where Ruby's alias does not. The strict targets refuse it
//! and name the builtin.

use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const PRESENTER: &str = r#"class NamePresenter
  attr_reader :name

  def initialize(name)
    @name = name
  end

  alias read_attribute_for_serialization send
  alias call_public public_send
  alias raw_send __send__
  alias label to_s
  alias describe inspect
  alias kind class
  alias me itself
  alias oid object_id
  alias digest hash
  alias same? equal?
  alias locked? frozen?
  alias a? is_a?
  alias kind_of_thing? kind_of?
  alias exactly? instance_of?
  alias answers? respond_to?
  alias none? nil?
  alias inspect inspect
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/presenters/name_presenter.rb", PRESENTER)
}

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
x = NamePresenter.new("Ada")
check("send", x.read_attribute_for_serialization(:name), "Ada")
check("send with a String", x.read_attribute_for_serialization("name"), "Ada")
check("public_send", x.call_public(:name), "Ada")
check("__send__", x.raw_send(:name), "Ada")
check("to_s", x.label, x.to_s)
check("inspect", x.describe.start_with?("#<NamePresenter"), true)
check("class", x.kind, NamePresenter)
check("itself", x.me.equal?(x), true)
check("object_id", x.oid, x.object_id)
check("hash", x.digest, x.hash)
check("equal? self", x.same?(x), true)
check("equal? other", x.same?(NamePresenter.new("Ada")), false)
check("frozen?", x.locked?, false)
check("is_a?", x.a?(NamePresenter), true)
check("kind_of?", x.kind_of_thing?(Object), true)
check("instance_of?", x.exactly?(Object), false)
check("respond_to? yes", x.answers?(:name), true)
check("respond_to? no", x.answers?(:nope), false)
check("nil?", x.none?, false)
check("alias inspect inspect is a no-op", x.inspect.start_with?("#<NamePresenter"), true)
check("respond_to? include_all", x.answers?(:initialize, true), true)
arity = begin
  x.label(1)
  "no error"
rescue ArgumentError
  "ArgumentError"
end
check("to_s keeps its arity", arity, "ArgumentError")
puts "builtin aliases passed"
"##;

#[test]
fn an_alias_of_a_builtin_answers_as_the_builtin_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("builtin aliases passed"), "{}", run.stdout);
}

/// Where something else defines the builtin's name, Ruby's alias captured
/// what stood at the alias (Ruby 4.0.7), and a delegating method can't
/// reproduce it, so each shape stays the gap:
/// - the class defines it after the alias (the alias keeps `Object#to_s`);
/// - a module included before (the alias is the module's) or after (the
///   alias keeps `Object#to_s` while `to_s` is the module's);
/// - a module prepended (the alias keeps `Object#to_s`);
/// - a superclass that defines it (the alias is the superclass's);
/// - a later `self.include`, `attr_reader :to_s`, `define_method(:to_s)` or
///   `alias to_s other` (the alias keeps `Object#to_s`).
#[test]
fn a_builtin_something_else_defines_stays_the_gap() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    let cases = [
        ("redefined_presenter", "class RedefinedPresenter\n  alias label to_s\n\n  def to_s\n    \"mine\"\n  end\nend\n"),
        ("include_before_presenter", "class IncludeBeforePresenter\n  include Loud\n  alias label to_s\nend\n"),
        ("include_after_presenter", "class IncludeAfterPresenter\n  alias label to_s\n  include Loud\nend\n"),
        ("prepend_presenter", "class PrependPresenter\n  alias label to_s\n  prepend Loud\nend\n"),
        ("child_presenter", "class ChildPresenter < BasePresenter\n  alias label to_s\nend\n"),
        ("self_include_presenter", "class SelfIncludePresenter\n  alias label to_s\n  self.include Loud\nend\n"),
        ("reader_presenter", "class ReaderPresenter\n  alias label to_s\n  attr_reader :to_s\nend\n"),
        ("defined_presenter", "class DefinedPresenter\n  alias label to_s\n  define_method(:to_s) { \"defined\" }\nend\n"),
        ("realiased_presenter", "class RealiasedPresenter\n  def shout\n    \"shout\"\n  end\n\n  alias label to_s\n  alias to_s shout\nend\n"),
    ];
    let mut tree: HashMap<PathBuf, Vec<u8>> = cases
        .iter()
        .map(|(file, body)| (PathBuf::from(format!("app/presenters/{file}.rb")), body.as_bytes().to_vec()))
        .collect();
    tree.insert(PathBuf::from("app/presenters/loud.rb"), b"module Loud\n  def to_s\n    \"loud\"\n  end\nend\n".to_vec());
    tree.insert(PathBuf::from("app/presenters/base_presenter.rb"), b"class BasePresenter\n  def to_s\n    \"base\"\n  end\nend\n".to_vec());
    tree.insert(PathBuf::from("app/presenters/name_presenter.rb"), PRESENTER.as_bytes().to_vec());
    tree.insert(PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec());
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let gaps: Vec<String> = roundhouse::ingest::survey::drain().into_iter().map(|g| g.to_string()).collect();
    for (file, _) in cases {
        assert!(gaps.iter().any(|g| g.contains(&format!("{file}.rb")) && g.contains("alias")), "{file}: {gaps:#?}");
    }
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "NamePresenter"), "{gaps:#?}");
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "BasePresenter"), "{gaps:#?}");
}

/// The strict targets have no `send` to delegate to: refused where it is
/// written, naming the builtin.
#[test]
fn a_strict_target_refuses_and_names_the_builtin() {
    let (_emitted, errors) = app().emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("`send`") && e.contains("read_attribute_for_serialization")), "{errors:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_alias_of_a_builtin_answers_as_the_builtin_on_spinel() {
    let run = app().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("builtin aliases passed"), "{}", run.stdout);
}
