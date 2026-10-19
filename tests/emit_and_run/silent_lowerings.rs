//! Behavioral pins for four silent lowerings.
//!
//! Each one can be deleted or simplified and leave `check` quiet while
//! the emitted program sums the wrong column, renders the wrong
//! partial, captures in the wrong order, or drops a password that a
//! later `valid?` still accepts. A one-hop `render partial:` through
//! the parent controller is already pinned in `emit_and_run.rs`; the
//! partial test here is the hop the one-level walk misses, and the
//! keyword form that must stay in its own directory.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  layout false\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
}

fn schema(tables: &str) -> String {
    format!("ActiveRecord::Schema.define do\n{tables}end\n")
}

#[test]
fn symbol_sum_totals_each_record_once_in_order() {
    let run = app()
        .write(
            "db/schema.rb",
            &schema(
                "  create_table \"articles\", force: :cascade do |t|\n    t.integer \"score\", default: 0, null: false\n  end\n",
            ),
        )
        .write(
            "app/models/article.rb",
            r#"class Article < ApplicationRecord
  def scored_total
    counted_pair.sum(:score)
  end

  def counted_pair
    self.score = score + 1
    [Article.new(score: 2), Article.new(score: 3)]
  end
end
"#,
        )
        .run_ruby(
            r#"
article = Article.new(score: 0)
total = article.scored_total
raise "sum #{total.inspect} receiver reads #{article.score}" unless total == 5 && article.score == 1
puts "symbol sum passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("symbol sum passed"), "{}", run.stdout);
}

#[test]
fn a_bare_partial_climbs_to_the_grandparent_and_an_own_partial_stays() {
    let run = app()
        .write("db/schema.rb", &schema(""))
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/notes\", to: \"mod_notes#index\"\nend\n",
        )
        .write("app/controllers/mod_controller.rb", "class ModController < ApplicationController\nend\n")
        .write(
            "app/controllers/mod_notes_controller.rb",
            "class ModNotesController < ModController\n  def index\n  end\nend\n",
        )
        .write(
            "app/views/mod_notes/index.html.erb",
            "<%= render partial: \"subnav\", locals: { label: \"grandparent\" } %>\n<%= render partial: \"kept\", locals: { label: \"stay\" } %>\n",
        )
        .write(
            "app/views/application/_subnav.html.erb",
            "<%# locals: (label:) %>\n<p id=\"chain-subnav\"><%= label %></p>\n",
        )
        .write(
            "app/views/mod_notes/_kept.html.erb",
            "<%# locals: (label:) %>\n<p id=\"own-kept\"><%= label %></p>\n",
        )
        .write(
            "app/views/mod/_kept.html.erb",
            "<%# locals: (label:) %>\n<p id=\"parent-kept\"><%= label %></p>\n",
        )
        .run_ruby(
            r#"
status, _headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/notes", "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
text = +""
body.each { |part| text << part }
raise "GET /notes answered #{status}: #{text}" unless status == 200
raise "bare subnav did not climb to application: #{text}" unless text.include?("<p id=\"chain-subnav\">grandparent</p>")
raise "own-dir kept partial was rewritten: #{text}" unless text.include?("<p id=\"own-kept\">stay</p>")
raise "parent kept partial rendered: #{text}" if text.include?("parent-kept")
puts "partial qualify passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("partial qualify passed"), "{}", run.stdout);
}

#[test]
fn capture_concats_in_order_without_evaluating_twice() {
    let run = app()
        .write("db/schema.rb", &schema(""))
        .write(
            "app/helpers/stories_helper.rb",
            r#"module StoriesHelper
  def line(left, right, include_right)
    seen = ""
    body = capture do
      concat(begin
        seen = seen + left.to_s
        left
      end)
      concat(begin
        seen = seen + right.to_s
        right
      end) if include_right
    end
    body.to_s + "|" + seen
  end

  def returned(value)
    capture { value }
  end
end
"#,
        )
        .run_ruby(
            r#"
both = StoriesHelper.line("A", "B", true)
raise "order or double evaluation: #{both.inspect}" unless both == "AB|AB"
skipped = StoriesHelper.line("A", "B", false)
raise "condition evaluated the skipped concat: #{skipped.inspect}" unless skipped == "A|A"
returned = StoriesHelper.returned("icon")
raise "capture dropped the block value: #{returned.inspect}" unless returned == "icon"
puts "capture passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("capture passed"), "{}", run.stdout);
}

#[test]
fn update_password_persists_while_valid_stays_true() {
    let run = app()
        .write(
            "db/schema.rb",
            &schema(
                "  create_table \"users\", force: :cascade do |t|\n    t.string \"password_digest\"\n  end\n",
            ),
        )
        .write(
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_secure_password

  def replace_password(secret)
    update(password: secret)
  end
end
"#,
        )
        .run_ruby(
            r#"
user = User.create!(password: "old-secret", password_confirmation: "old-secret")
user = User.find(user.id)
raise "setup invalid" unless user.valid? && user.authenticate("old-secret")
ok = user.replace_password("new-secret")
raise "update returned #{ok.inspect} valid=#{user.valid?}" unless ok && user.valid?
saved = User.find(user.id)
raise "valid? went red after reload" unless saved.valid?
raise "password dropped while valid? stayed true" unless saved.authenticate("new-secret")
raise "old password still authenticates" if saved.authenticate("old-secret")
puts "password update passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("password update passed"), "{}", run.stdout);
}
