//! A model method with a keyword beside a `**rest` (`def opts(name:,
//! **rest)`) was emitted as `def opts(name:, rest = {})`: a positional
//! after a keyword, which CRuby refuses to parse, so the whole model
//! file failed to load. The emitted program has to keep the `**rest`
//! and bind the extra keywords into it. (Kept out of
//! tests/emit_and_run.rs so concurrent appends there do not conflict;
//! same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The issue's exact shape: a required keyword beside the `**rest`.
/// The emitted program must bind the named keyword and collect the
/// extras — and return `nil` for an extra that was never passed.
#[test]
fn a_model_keyword_rest_beside_a_required_keyword_collects_the_extras() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def opts(name:, **rest)\n    rest[:x]\n  end\n",
        )
        .run_ruby(
            r#"
got = Article.new.opts(name: "a", x: 1)
raise "extra keyword lost: #{got.inspect}" unless got == 1
raise "absent extra not nil" unless Article.new.opts(name: "a").nil?
puts "kwrest ok"
"#,
        )
        .assert_passes();
}

/// An optional keyword beside the `**rest`. The library-class path may
/// flatten this shape (`keeps_keywords`); the model path keeps the
/// keyword real, so the kwrest must stay too — with the default still
/// binding when the keyword is omitted.
#[test]
fn a_model_keyword_rest_beside_an_optional_keyword_collects_the_extras() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def label(prefix: \"item\", **rest)\n    \"#{prefix}:#{rest[:x]}\"\n  end\n",
        )
        .run_ruby(
            r#"
raise "explicit prefix lost" unless Article.new.label(prefix: "p", x: 2) == "p:2"
raise "default prefix lost" unless Article.new.label(x: 3) == "item:3"
puts "kwrest ok"
"#,
        )
        .assert_passes();
}
