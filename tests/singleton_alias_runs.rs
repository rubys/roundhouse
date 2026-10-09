//! `alias new old` inside `class << self` (Mastodon's
//! `DomainBlock.blocked?`, an alias of `suspend?`). The singleton-class
//! walk took `alias_method` but not the `alias` keyword, so the whole
//! model was an ingest gap. (Kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict; same harness.)

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn an_alias_inside_class_self_copies_the_class_method() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  class << self\n    def titled?(title)\n      where(title: title).exists?\n    end\n\n    alias known? titled?\n  end\n",
        )
        .run_ruby(
            r#"
title = Article.create!(title: "Alias probe", body: "A body long enough to validate.").title
raise "alias lost: #{Article.known?(title).inspect}" unless Article.known?(title) == true
raise "alias answers for a missing title" unless Article.known?("no such title") == false
puts "singleton alias ok"
"#,
        )
        .assert_passes();
}

#[test]
fn an_alias_inside_class_self_of_an_undefined_method_stays_a_gap() {
    let files = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  class << self\n    alias known? exists?\n  end\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
    ];
    let err = roundhouse::ingest::ingest_app_from_tree(
        files.into_iter().map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec())).collect(),
    )
    .err()
    .expect("an alias of a method the block does not define must not ingest");
    assert!(err.to_string().contains("`alias` of a method the block does not define"), "{err}");
}
