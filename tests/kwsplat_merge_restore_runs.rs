//! `opts(name: t, **h)` into `def opts(name:, **rest)`: ingest desugars
//! the call site to the merge chain `{ name: t }.merge(h)`, which emits
//! as one positional Hash. CRuby then raises `wrong number of arguments
//! (given 1, expected 0; required keyword: name)` while `check` is
//! clean. `lower::kwsplat`'s `restore_kwrest_splat` has to put the `**`
//! back for the merge forms too, not only for a plain read. (Kept out
//! of tests/emit_and_run.rs so concurrent appends there do not
//! conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const MODEL: &str = "class Article < ApplicationRecord
  def opts(name:, **rest)
    \"#{name}:#{rest[:x]}:#{rest[:name].inspect}\"
  end

  def literal_first(t, h)
    opts(name: t, **h)
  end

  def bundle_first(t, h)
    opts(**h, name: t)
  end
";

/// Both merge orders: the literal before the splat (the bundle's key
/// wins, as Ruby's `**` does) and after it (the literal wins).
#[test]
fn a_keyword_beside_a_double_splat_runs_against_a_keyword_rest() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            MODEL,
        )
        .run_ruby(
            r#"
a = Article.new
got = a.literal_first("t", { x: 1 })
raise "literal-first: #{got.inspect}" unless got == "t:1:nil"
got = a.bundle_first("t", { x: 2 })
raise "bundle-first: #{got.inspect}" unless got == "t:2:nil"
got = a.literal_first("t", { name: "h" })
raise "a later splat's key must win: #{got.inspect}" unless got == "h::nil"
got = a.bundle_first("t", { name: "h" })
raise "a later literal must win: #{got.inspect}" unless got == "t::nil"
puts "kwsplat merge ok"
"#,
        )
        .assert_passes();
}
