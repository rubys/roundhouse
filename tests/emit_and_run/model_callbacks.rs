//! `define_model_callbacks`, `define_callbacks`/`set_callback`/`skip_callback` and `run_callbacks`, each answer ActiveRecord 8.1.4's.

use super::emit_and_run;

const CUSTOM: &str = r##"  LOG = []
  define_model_callbacks :archive
  define_model_callbacks :publish, only: [:before, :after]
  before_archive :b1
  before_archive :b2, if: :flag?
  after_archive :a1
  after_archive :a2
  after_archive { LOG << "ablock" }
  around_archive :ar1
  around_archive :ar2
  before_publish :halt_if_bad
  after_publish :pub_after
  attr_accessor :flag, :bad
  def flag? = flag
  def b1 = LOG << "b1"
  def b2 = LOG << "b2"
  def a1 = LOG << "a1"
  def a2 = LOG << "a2"
  def ar1
    LOG << "ar1-in"
    yield
    LOG << "ar1-out"
  end
  def ar2
    LOG << "ar2-in"
    yield
    LOG << "ar2-out"
  end
  def halt_if_bad
    throw(:abort) if bad
    LOG << "pub-before"
  end
  def pub_after = LOG << "pub-after"
  def archive = run_callbacks(:archive) { LOG << "body"; :done }
  def publish = run_callbacks(:publish) { LOG << "pub-body"; :pub }
  define_model_callbacks :review
  before_review :guard
  attr_accessor :stopped
  def guard = stop!
  def stop! = (throw(:abort) if stopped)
  def review = run_callbacks(:review) { LOG << "review"; :rev }
"##;

#[test]
fn a_model_callback_chain_runs_as_rails_runs_it() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{CUSTOM}"))
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
l = Article::LOG
i = Article.new(title: "t", body: "body number one is long enough")
i.flag = true
check("all links", [i.archive, l.dup], [:done, ["b1", "b2", "ar1-in", "ar2-in", "body", "ar2-out", "ar1-out", "a1", "a2", "ablock"]]); l.clear
i.flag = false
check("if: false", [i.archive, l.dup], [:done, ["b1", "ar1-in", "ar2-in", "body", "ar2-out", "ar1-out", "a1", "a2", "ablock"]]); l.clear
check("only:", [i.publish, l.dup], [:pub, ["pub-before", "pub-body", "pub-after"]]); l.clear
i.bad = true
check("throw :abort", [i.publish, l.dup], [false, []])
check("no abort through a helper", [i.review, l.dup], [:rev, ["review"]]); l.clear
i.stopped = true
check("abort through a helper", [i.review, l.dup], [false, []])
puts "ok"
"##,
        )
        .assert_passes();
}

#[test]
fn run_callbacks_on_save_and_a_raw_chain_with_a_subclass_skip() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  LOG = []\n  before_save :bs\n  after_save :as\n  define_callbacks :perform\n  \
             set_callback :perform, :before, :x1\n  set_callback :perform, :after, :y1\n  set_callback :perform, :after, :y2\n  \
             attr_accessor :halt\n  def bs\n    LOG << \"bs\"\n    throw(:abort) if halt\n  end\n  def as = LOG << \"as\"\n  def x1 = LOG << \"x1\"\n  def y1 = LOG << \"y1\"\n  def y2 = LOG << \"y2\"\n  \
             def touch_it = run_callbacks(:save) { LOG << \"manual\"; 7 }\n  def perform = run_callbacks(:perform) { LOG << \"perform\"; 42 }\n",
        )
        .write("app/models/special.rb", "class Special < Article\n  skip_callback :perform, :after, :y1\nend\n")
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|\n", "create_table \"articles\", force: :cascade do |t|\n    t.string \"type\"\n")
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
l = Article::LOG
a = Article.new(title: "t", body: "body number one is long enough")
check("run_callbacks(:save)", [a.touch_it, l.dup], [7, ["bs", "manual", "as"]]); l.clear
check("set_callback afters", [a.perform, l.dup], [42, ["x1", "perform", "y2", "y1"]]); l.clear
check("skip_callback", [Special.new.perform, l.dup], [42, ["x1", "perform", "y2"]]); l.clear
a.halt = true
check("run_callbacks(:save) halted", [a.touch_it, l.dup], [false, ["bs"]])
puts "ok"
"##,
        )
        .assert_passes();
}

#[test]
fn discards_callbacks_join_the_generic_chain() {
    emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    discard (2.0.0)\n      activerecord (>= 5.0, < 9.0)\n    debug (1.11.1)\n")
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|\n", "create_table \"articles\", force: :cascade do |t|\n    t.datetime \"discarded_at\"\n")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Discard::Model\n  LOG = []\n  attr_accessor :keep\n  before_discard :check\n  around_discard :wrap\n  \
             after_discard :note, if: -> { title != \"quiet\" }\n  after_undiscard { LOG << \"undiscarded #{title}\" }\n  \
             def check\n    LOG << \"check\"\n    throw(:abort) if keep\n  end\n  \
             def wrap\n    LOG << \"wrap-in\"\n    yield\n    LOG << \"wrap-out\"\n  end\n  def note = LOG << \"note\"\n",
        )
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
l = Article::LOG
a = Article.create!(title: "a", body: "body number one is long enough")
check("discard", [a.discard, a.discarded?, l.dup], [true, true, ["check", "wrap-in", "wrap-out", "note"]]); l.clear
check("again", [a.discard, l.dup], [false, []]); l.clear
check("undiscard", [a.undiscard, a.discarded?, l.dup], [true, false, ["undiscarded a"]]); l.clear
b = Article.create!(title: "quiet", body: "body number one is long enough")
check("if: false", [b.discard, l.dup], [true, ["check", "wrap-in", "wrap-out"]]); l.clear
c = Article.create!(title: "c", body: "body number one is long enough")
c.keep = true
check("halted", [c.discard, c.discarded?, l.dup], [false, false, ["check"]])
puts "ok"
"##,
        )
        .assert_passes();
}
