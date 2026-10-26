//! `delegate …, allow_nil: true`. ActiveSupport 8.1.4 (oracle
//! `delegate_allow_nil.rb`) generates `_ = target; _.m if !_.nil? ||
//! nil.respond_to?(:m)`: a nil target answers nil, a `false` target is
//! called through (`NoMethodError`), and a method nil itself answers
//! (`to_s`, `presence`, …) is called on nil. Without allow_nil a nil
//! target raises `ActiveSupport::DelegationError`.
//!
//! An `allow_nil: true` delegation was never lowered, on a model or a
//! plain class. It is now, except for a name nil answers (kept a gap:
//! Rails calls it on nil) and the shapes plain delegation already keeps.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const COMMENT: &str = "class Comment < ApplicationRecord\n  belongs_to :article\n";
const DELEGATES: &str = "class Comment < ApplicationRecord\n  belongs_to :article, optional: true\n  delegate :title, to: :article, allow_nil: true\n  delegate :title, to: :article, prefix: true, allow_nil: true\n  delegate :title, to: :article, prefix: :parent, allow_nil: true\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
article = Article.create!(title: "Delegated", body: "A body long enough to validate.")
with = Comment.new(article: article, commenter: "c", body: "b")
check("present", with.title, "Delegated")
check("present, prefix true", with.article_title, "Delegated")
check("present, prefix symbol", with.parent_title, "Delegated")
without = Comment.new(commenter: "c", body: "b")
check("nil target", without.title, nil)
check("nil target, prefix true", without.article_title, nil)
check("nil target, prefix symbol", without.parent_title, nil)
puts "delegate allow_nil passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit("app/models/comment.rb", COMMENT, DELEGATES)
}

#[test]
fn a_models_allow_nil_delegate_answers_nil_for_a_nil_target_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("delegate allow_nil passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_models_allow_nil_delegate_answers_nil_for_a_nil_target_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("delegate allow_nil passed"), "{}", run.stdout);
}

fn warnings_for(delegates: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("roundhouse-delegate-allow-nil-{}-{}", std::process::id(), delegates.len()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let model = std::fs::read_to_string(source.join("app/models/comment.rb")).unwrap().replacen(COMMENT, delegates, 1);
    std::fs::write(source.join("app/models/comment.rb"), model).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--survey", "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&root).ok();
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.contains("delegate")).map(String::from).collect()
}

/// A `false` target is called through, as Rails' `!_.nil?` guard does
/// (Rails raises `NoMethodError`); the forwarder does not answer `false`
/// for it, so analysis reports the call on a Boolean where it is written.
#[test]
fn a_false_target_is_called_through() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            COMMENT,
            "class Comment < ApplicationRecord\n  belongs_to :article\n  delegate :body, to: :falsy, prefix: :never, allow_nil: true\n\n  def falsy\n    false\n  end\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("no known method `body` on bool")), "{errors:#?}");
}

/// A name nil answers (`presence`, an ActiveSupport method) is called on
/// nil by Rails, so it stays a gap.
#[test]
fn a_name_nil_answers_stays_a_gap() {
    let warnings = warnings_for("class Comment < ApplicationRecord\n  belongs_to :article\n  delegate :presence, to: :article, allow_nil: true\n");
    assert!(warnings.iter().any(|w| w.contains("delegate not supported")), "{warnings:#?}");
}

/// A delegated name the class calls with arguments or a block stays a gap,
/// as without allow_nil.
#[test]
fn a_name_called_with_arguments_stays_a_gap() {
    let warnings = warnings_for("class Comment < ApplicationRecord\n  belongs_to :article\n  delegate :title, to: :article, allow_nil: true\n\n  def shout\n    title(1) { 2 }\n  end\n");
    assert!(warnings.iter().any(|w| w.contains("delegate not supported")), "{warnings:#?}");
}

/// The same on a plain class (`ingest::delegate`, the library side).
#[test]
fn a_plain_classs_allow_nil_delegate_on_ruby() {
    const LIB: &str = "class Holder\n  attr_reader :target\n\n  def initialize(target)\n    @target = target\n  end\n\n  delegate :upcase, to: :target, allow_nil: true\nend\n";
    let run = emit_and_run::real_blog()
        .write("app/lib/holder.rb", LIB)
        .run_ruby("require_relative \"app/models/holder\"\np [Holder.new(\"ab\").upcase, Holder.new(nil).upcase]\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[\"AB\", nil]\n");
}

/// An ordinary delegation to a PRIVATE target named like the allow_nil
/// forwarder's local is read through `self.`, as Rails reads its own
/// reserved names (`arg`, `value`, …): Ruby 2.7+ allows a private method
/// with a literal `self` receiver (Ruby 4.0.7 oracle `private_self_call.rb`).
#[test]
fn an_ordinary_delegation_to_a_private_target_named_like_the_local_runs() {
    const LIB: &str = "class Holder\n  delegate :upcase, to: :__allow_nil_target__\n\n  private\n\n  def __allow_nil_target__\n    \"ab\"\n  end\nend\n";
    let run = emit_and_run::real_blog()
        .write("app/lib/holder.rb", LIB)
        .run_ruby("require_relative \"app/models/holder\"\np Holder.new.upcase\n");
    run.assert_passes();
    assert_eq!(run.stdout, "\"AB\"\n");
}
