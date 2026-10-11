//! `defined?(Const)` guards that the tree, lockfile and application.rb
//! decide are folded away before emit; the live branch must still run
//! and the dead one must not, in the emitted Ruby. The spinel gate
//! must stop refusing the folded `defined?`.

use roundhouse::project::BuildTarget;

use super::emit_and_run;

const SOURCE: &str = r#"class DefinedProbe
  def self.run
    log = []
    log << :storage if defined?(ActiveStorage)
    log << :tenant if defined? ActsAsTenant
    log << :self_class if defined?(::DefinedProbe)
    log << :nested_missing if defined?(DefinedProbe::Missing)
    log << :logger if defined?(Rails) && Rails.respond_to?(:logger) && Rails.logger
    log << :referral if defined?(::Refer)
    log
  end
end
"#;

const ASSERTIONS: &str = r#"
got = DefinedProbe.run
raise "defined? guards ran #{got.inspect}" unless got == [:storage, :self_class, :logger]
puts "defined guards passed"
"#;

#[test]
fn folded_defined_guards_run_the_live_branch_and_skip_the_dead_one() {
    let run = emit_and_run::real_blog()
        .write("app/models/defined_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("defined guards passed"));
}

#[test]
fn spinel_no_longer_refuses_a_folded_defined() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/models/defined_probe.rb", SOURCE)
        .emit(BuildTarget::Spinel);
    let refused: Vec<_> = errors.iter().filter(|e| e.contains("defined?")).collect();
    // `defined?(DefinedProbe::Missing)`: the prefix resolves, the tail
    // is unprovable, so exactly that one stays refused.
    assert_eq!(refused.len(), 1, "{refused:#?}");
}

const GUARDED_MODEL: &str = "class Article < ApplicationRecord\n  has_one_attached :cover if defined?(ActiveStorage)\n  has_many_attached :gallery if defined?(ActsAsTenant)\n";

#[test]
fn guarded_attachment_macros_follow_their_guard() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", GUARDED_MODEL)
        .run_ruby(r#"
a = Article.new
raise "true guard did not declare cover" unless a.respond_to?(:cover) && a.respond_to?(:cover=)
raise "false guard declared gallery" if a.respond_to?(:gallery)
puts "guarded attachments passed"
"#)
        .assert_passes();
}

#[test]
fn a_false_guarded_filter_is_never_installed() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  before_action :set_article, only: %i[ show edit update destroy ]\n",
            "  before_action :set_article, only: %i[ show edit update destroy ]\n  before_action :deny if defined?(ActsAsTenant)\n\n  def deny\n    head :internal_server_error\n  end\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn a_true_guarded_filter_stays_refused_rather_than_vanishing() {
    // The filter is classified at ingest, before the fold; hoisting its
    // bare form would silently never install it. The error stays.
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  before_action :set_article, only: %i[ show edit update destroy ]\n",
            "  before_action :set_article, only: %i[ show edit update destroy ]\n  before_action :audit if defined?(ActiveStorage)\n\n  def audit\n  end\n",
        )
        .emit(BuildTarget::Spinel);
    assert_eq!(errors.iter().filter(|e| e.contains("defined?")).count(), 1, "{errors:#?}");
}
