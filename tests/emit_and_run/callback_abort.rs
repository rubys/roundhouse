//! `throw :abort` from a before_* callback halts `save` / `destroy` as ActiveRecord 8.1.4 does (false, RecordNotSaved, RecordNotDestroyed); from an after_* one it escapes, rolled back.

use super::emit_and_run;

const CALLBACKS: &str = r##"  LOG = []
  attr_accessor :stop_at
  before_validation { LOG << "bv"; throw(:abort) if stop_at == :bv }
  after_validation { LOG << "av" if stop_at == :invalid }
  before_save { LOG << "bs"; helper_stop if stop_at == :bs }
  before_create { LOG << "bc"; throw(:abort) if stop_at == :bc }
  before_update { LOG << "bu"; throw(:abort) if stop_at == :bu }
  before_destroy { LOG << "bd"; throw(:abort) if stop_at == :bd }
  after_save { LOG << "as"; throw(:abort) if stop_at == :as }
  after_destroy { LOG << "ad" }
  def helper_stop = throw(:abort)
"##;

#[test]
fn a_callback_that_throws_abort_halts_save_and_destroy() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{CALLBACKS}"))
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
def run
  Article::LOG.clear
  r = begin
    yield
  rescue => e
    "#{e.class}: #{e.message}"
  end
  [r.is_a?(Article) ? :record : r, Article::LOG.dup]
end
body = "body number one is long enough"
base = Article.count
check("save bv", run { a = Article.new(title: "x", body: body); a.stop_at = :bv; a.save }, [false, ["bv"]])
check("save! bv", run { a = Article.new(title: "x", body: body); a.stop_at = :bv; a.save! }, ["ActiveRecord::RecordInvalid: Validation failed: ", ["bv"]])
check("save bs", run { a = Article.new(title: "x", body: body); a.stop_at = :bs; a.save }, [false, ["bv", "bs"]])
check("save! bs", run { a = Article.new(title: "x", body: body); a.stop_at = :bs; a.save! }, ["ActiveRecord::RecordNotSaved: Failed to save the record", ["bv", "bs"]])
check("save bc", run { a = Article.new(title: "x", body: body); a.stop_at = :bc; a.save }, [false, ["bv", "bs", "bc"]])
check("after_save as", run { a = Article.new(title: "x", body: body); a.stop_at = :as; a.save }, ["UncaughtThrowError: uncaught throw :abort", ["bv", "bs", "bc", "as"]])
check("nothing inserted", Article.count, base)
check("invalid runs after_validation", run { a = Article.new(title: "", body: body); a.stop_at = :invalid; a.save }, [false, ["bv", "av"]])
check("valid? bv", run { a = Article.new(title: "x", body: body); a.stop_at = :bv; [a.valid?, a.errors.to_a] }, [[false, []], ["bv"]])
check("valid? invalid runs after_validation", run { a = Article.new(title: "", body: body); a.stop_at = :invalid; a.valid? }, [false, ["bv", "av"]])
check("bv halt clears earlier errors", run { a = Article.new(title: "", body: body); a.save; a.stop_at = :bv; a.save! }, ["ActiveRecord::RecordInvalid: Validation failed: ", ["bv", "bv"]])
a = Article.create!(title: "u", body: body)
check("update bu", run { a.stop_at = :bu; a.title = "v"; a.save }, [false, ["bv", "bs", "bu"]])
check("update! bu", run { a.save! }, ["ActiveRecord::RecordNotSaved: Failed to save the record", ["bv", "bs", "bu"]])
check("not updated", Article.find(a.id).title, "u")
check("update_attribute bs", run { a.stop_at = :bs; a.update_attribute(:title, "w") }, [false, ["bs"]])
check("update_attribute rolled back", Article.find(a.id).title, "u")
check("destroy bd", run { a.stop_at = :bd; a.destroy }, [false, ["bd"]])
check("destroy! bd", run { a.destroy! }, ["ActiveRecord::RecordNotDestroyed: Failed to destroy Article with id=#{a.id}", ["bd"]])
check("not destroyed", [a.persisted?, Article.exists?(a.id)], [true, true])
a.stop_at = nil
check("destroy", run { a.destroy }, [:record, ["bd", "ad"]])
puts "ok"
"##,
        )
        .assert_passes();
}
