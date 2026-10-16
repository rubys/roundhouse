//! A relation-taking class method with a REST param, run for real under
//! CRuby — direct, zero-arg, splatted, and chained calls (including a
//! chained zero-arg call, the shape that used to pad a spurious `nil`
//! into the rest slot, and an in-scope call threading `__rel: __rel`).
//! `tests/relation_rest_param_class_method.rs` checks the emitted
//! TEXT; this checks the emitted CODE actually RUNS and answers the
//! rows Rails would.
//!
//! Skips when `ruby` with the sqlite3 gem is not on the machine; the CI
//! core job has it.

use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

fn tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-rel-rest-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"tag\"\n    t.boolean \"active\", null: false\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  scope :active, -> { where(active: true) }
  scope :combo, -> { those_tagged("a") }

  def self.those_tagged(*tags)
    where(tag: tags)
  end

  # Wrapper class methods so each call shape goes through the SAME
  # ingest/analyze/lower pipeline a controller or view body would —
  # `rewrite_call_site` runs over every method body in the app, model
  # methods included, gated on `mentions_scope` seeing a registered
  # scope name (`those_tagged` is one) anywhere in the body.
  def self.direct_two(a, b)
    those_tagged(a, b)
  end

  def self.direct_zero
    those_tagged
  end

  def self.direct_splat(tags)
    those_tagged(*tags)
  end

  def self.chained_one(tag)
    active.those_tagged(tag)
  end

  def self.chained_zero
    active.those_tagged
  end

  def self.chained_splat(tags)
    active.those_tagged(*tags)
  end
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.active\n  end\nend\n",
        ),
        ("app/views/widgets/index.html.erb", "<%= @widgets.length %>\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        ),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let mut app = ingest_app(&dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::target_files(&app, &dir, BuildTarget::Ruby).expect("files");
    let out = dir.join("emitted");
    roundhouse::project::write_to_dir(&files, &out).expect("write");
    out
}

#[test]
fn direct_zero_arg_splat_and_chained_calls_answer_like_rails() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = tree();
    // Seed: #1 tag=a active=true, #2 tag=a active=false, #3 tag=b
    // active=true, #4 tag=c active=true.
    let script = r#"
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
[["a", true], ["a", false], ["b", true], ["c", true]].each do |tag, active|
  w = Widget.new; w.tag = tag; w.active = active; w.save!
end

# Direct: no relation involved at all.
p Widget.direct_two("a", "b").length                     # a,a,b -> 3
# Zero-arg direct: tag IN () matches nothing, same as Rails.
p Widget.direct_zero.length                              # 0
# Splat direct: same rows as the direct two-arg call.
p Widget.direct_splat(["a", "b"]).length                  # 3

# Chained: the active scope narrows first.
p Widget.chained_one("a").length                         # a&active -> 1
# Chained zero-arg: the shape that used to pad a spurious nil into
# *tags before the fix — must still run, and tag IN () still answers 0.
p Widget.chained_zero.length                             # 0
# Chained splat: active AND tag IN (a, b).
p Widget.chained_splat(["a", "b"]).length                 # a&active, b&active -> 2

# In-scope: `combo`'s own body calls `those_tagged("a")` with its own
# __rel (a fresh Relation.new(self), no active filter) threaded in by
# keyword rather than re-seeding.
p Widget.combo.length                                    # tag=a, any active -> 2
"#;
    let result = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&out)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "3\n0\n3\n1\n0\n2\n2\n"
    );
}
