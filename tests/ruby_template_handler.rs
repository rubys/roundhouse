//! A `.ruby` view: Rails' `register_template_handler :ruby,
//! :source.to_proc`. The file is Ruby whose value is the rendered body,
//! output unescaped (an XML/RSS document built in plain Ruby). It was a
//! skipped engine ("view template not ingested: ruby"), so strict
//! ingest refused the whole app.
//!
//! Now it is ingested like the other engines: the source is the value
//! of one `raw(( … ))` output, its ivars become the view's parameters,
//! and the `format.rss` branch reaches it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const CONTROLLER: &str = r#"class FeedsController < ApplicationController
  def show
    @items = %w[a b]
    respond_to do |format|
      format.rss { render action: "show", layout: false }
    end
  end

  def hello
    @name = "W"
  end
end
"#;

const FEED: &str = r#"lines = @items.map { |i| "<item>#{i}</item>" }
"<feed>" + lines.join + "</feed>"
"#;

const HELLO: &str = "\"Hello & <b>#{@name}</b>\"\n";

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/feeds_controller.rb", CONTROLLER),
        ("app/views/feeds/show.rss.ruby", FEED),
        ("app/views/feeds/hello.html.ruby", HELLO),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/feed.rss\" => \"feeds#show\", :format => \"rss\"\n  get \"/hello\" => \"feeds#hello\"\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("strict ingest accepts `.ruby` views");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn views(app: &roundhouse::App) -> Vec<(String, String)> {
    ruby::emit_lowered_views(app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect()
}

fn find<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ends with {suffix}; got {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            )
        })
}

#[test]
fn both_templates_are_ingested_as_views() {
    let app = app();
    for name in ["feeds/show", "feeds/hello"] {
        assert!(
            app.views.iter().any(|v| v.name.as_str() == name),
            "{name} is ingested: {:?}",
            app.views.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
}

/// The ivar is a parameter, the body reads it as a local, and the rss
/// branch calls the view with its mime type.
#[test]
fn the_rss_branch_renders_the_ruby_view() {
    let app = app();
    let views = views(&app);
    let feed = find(&views, "app/views/feeds/show_rss.rb");
    assert!(feed.contains("def self.show_rss(items"), "{feed}");
    assert!(feed.contains("items.map"), "{feed}");
    assert!(!feed.contains("@items"), "no ivar survives in the module function:\n{feed}");

    let controller = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("feeds_controller.rb"))
        .map(|f| f.content)
        .expect("controller");
    assert!(controller.contains("Views::Feeds.show_rss(@items"), "{controller}");
    assert!(controller.contains("application/rss+xml"), "{controller}");
}

/// Render both views on CRuby and compare with what Rails 8.1.4
/// answers for the same ivars: the value of the source, unescaped.
#[test]
fn the_templates_render_what_rails_renders() {
    let app = app();
    let views = views(&app);
    let dir = std::env::temp_dir().join(format!("roundhouse-ruby-template-handler-{}", std::process::id()));
    let mut requires = String::new();
    for suffix in ["app/views/feeds/show_rss.rb", "app/views/feeds/hello.rb"] {
        let file = dir.join(suffix);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, find(&views, suffix)).unwrap();
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    // Each view opens with `require_relative "../../views"`.
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let helpers = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/action_view/view_helpers.rb");
    let script = format!(
        r#"require {helpers:?}
module ViewBufferCap
  def self.alloc(_key) = String.new
  def self.store(_key, _size) = nil
end
{requires}puts Views::Feeds.show_rss(%w[a b])
puts Views::Feeds.hello("W")
"#,
        helpers = helpers.display().to_string()
    );
    let run = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "<feed><item>a</item><item>b</item></feed>\nHello & <b>W</b>\n"
    );
}
