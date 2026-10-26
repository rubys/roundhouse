//! Emitted-program regression (kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict). Same harness.
//!
//! A model's `initialize(attrs = {})` default lowers to
//! `ActiveRecord::Base::EMPTY_ATTRS`, and the constructor skips
//! `after_initialize` when `attrs.equal?(ActiveRecord::Base::HYDRATE_ATTRS)`
//! (a hydration factory fires the hook itself once the columns are
//! set). On CRuby a literal `{}.freeze` is one shared frozen empty Hash,
//! so two constants written that way are the same object, and a bare
//! `Model.new` was taken for hydration: the hook never ran.
//!
//! Rails 8.1.4 (measured, a hook appending "!" to a string column):
//! `new` gives "!", `new(title: "x")` gives "x!", `create!(title:
//! "Hooked")` saves "Hooked!", and `find` / `all` read it back as
//! "Hooked!!" — the hook runs exactly once per construction or load.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_bare_new_runs_after_initialize_and_a_loaded_record_runs_it_once() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true",
            "  after_initialize do\n    self.title = \"#{title}!\"\n  end\n\n  validates :title, presence: true",
        )
        .run_ruby(r#"
bare = Article.new.title
raise "bare new: title #{bare.inspect}, want \"!\" (after_initialize once)" unless bare == "!"
with_attrs = Article.new(title: "x").title
raise "new(attrs): title #{with_attrs.inspect}, want \"x!\"" unless with_attrs == "x!"
Article.delete_all
a = Article.create!(title: "Hooked", body: "Long enough body")
raise "create!: title #{a.title.inspect}, want \"Hooked!\"" unless a.title == "Hooked!"
found = Article.find(a.id).title
raise "find: title #{found.inspect}, want \"Hooked!!\"" unless found == "Hooked!!"
all = Article.all.map(&:title)
raise "all: titles #{all.inspect}, want [\"Hooked!!\"]" unless all == ["Hooked!!"]
raise "sentinels share one object" if ActiveRecord::Base::HYDRATE_ATTRS.equal?(ActiveRecord::Base::EMPTY_ATTRS)
puts "after_initialize once"
"#)
        .assert_passes();
}
