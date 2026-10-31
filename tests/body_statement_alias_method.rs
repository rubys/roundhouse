//! A library class body's `alias_method` of a method the body does not
//! define itself runs on the Ruby tree only when an app ancestor defines
//! that method (`ingest::body_statements`). One whose method only a
//! macro defines stays dropped, so the class still loads.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BASE: &str = "class Token\n  def sub = \"s-1\"\n\n  def self.kind = \"token\"\nend\n";
const SUB: &str = "class IdToken < Token\n  alias_method :subject, :sub\n\n  class << self\n    alias_method :category, :kind\n  end\nend\n";
const MACRO: &str = "module AttrRequired\n  def self.included(base)\n    base.extend(ClassMethods)\n  end\n\n  module ClassMethods\n    def attr_required(*names)\n      names.each { |n| attr_accessor(n) }\n    end\n  end\nend\n";
const HOOKED: &str = "class Claims\n  include AttrRequired\n  attr_required :sub\n  alias_method :subject, :sub\n\n  def label = \"claims\"\nend\n";

const SCRIPT: &str = "puts IdToken.new.subject\nputs IdToken.category\nputs Claims.new.label\n";

#[test]
fn an_alias_of_an_ancestors_method_runs_and_one_of_a_macros_method_is_dropped() {
    let run = emit_and_run::empty_app()
        .write("app/models/token.rb", BASE)
        .write("app/models/id_token.rb", SUB)
        .write("app/models/attr_required.rb", MACRO)
        .write("app/models/claims.rb", HOOKED)
        .run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    // CRuby 4.0 runs SCRIPT the same way on the source as written.
    assert_eq!(run.stdout.lines().collect::<Vec<_>>(), ["s-1", "token", "claims"], "{}", run.stderr);
}
