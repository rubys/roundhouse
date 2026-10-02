//! An optional keyword named after a reserved word keeps the keyword
//! group: `def f(next = nil)` does not parse, `def f(next: nil)` does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
#[allow(dead_code)]
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

#[allow(dead_code)]
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

#[allow(dead_code)]
fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const CLIENT: &str = r#"class BillingClient
  def invoices(customer_id: nil, next: nil, limit: 50)
    { customer_id: customer_id, limit: limit }
  end
end
"#;

#[test]
fn a_reserved_word_keyword_stays_a_keyword() {
    let files = spinel(&[
        ("app/services/billing_client.rb", CLIENT),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]);
    assert_parses(&files, "app/models/billing_client.rb");
    let emitted = file(&files, "app/models/billing_client.rb");
    assert!(emitted.contains("def invoices(customer_id: nil, next: nil, limit: 50)"), "{emitted}");
}

const POSTS_APP: &[(&str, &str)] = &[
    (
        "db/schema.rb",
        "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n",
    ),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  resources :posts, only: %i[index]\nend\n",
    ),
    ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
    (
        "app/controllers/posts_controller.rb",
        "class PostsController < ApplicationController\n  def index\n    @posts = Post.all\n  end\nend\n",
    ),
    ("app/views/layouts/application.html.erb", "<html><body><%= yield %></body></html>\n"),
];

/// The spinel tree for `POSTS_APP` plus `files`.
fn posts_app(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut all = POSTS_APP.to_vec();
    all.extend_from_slice(files);
    spinel(&all)
}

/// B1: the `class:` shorthand reads the keyword as a bare `class`,
/// which does not parse.
#[test]
fn a_helper_reads_a_reserved_word_keyword_with_the_shorthand() {
    let files = posts_app(&[
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def badge(text, class: \"badge\")\n    tag.span(text, class:)\n  end\nend\n",
        ),
        ("app/views/posts/index.html.erb", "<%= badge(\"hi\", class: \"big\") %>\n"),
    ]);
    assert_parses(&files, "app/models/application_helper.rb");
}

/// B1, the Nexus shape: the shorthand inside a Hash that is splatted
/// into the call.
#[test]
fn a_helper_merges_a_reserved_word_keyword_with_the_shorthand() {
    let files = posts_app(&[
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def badge(text, class: \"badge\", **options)\n    tag.span(text, **options.merge(class:))\n  end\nend\n",
        ),
        ("app/views/posts/index.html.erb", "<%= badge(\"hi\", class: \"big\", id: \"b\") %>\n"),
    ]);
    assert_parses(&files, "app/models/application_helper.rb");
}

/// B2, the Nexus shape: strict locals named after reserved words, read
/// with `local_assigns`. The first local became a positional `for`.
#[test]
fn a_partial_declares_reserved_word_strict_locals_read_with_local_assigns() {
    let files = posts_app(&[
        (
            "app/views/posts/_empty.html.erb",
            "<%# locals: (for:, class: \"\") %>\n<p class=\"<%= local_assigns[:class] %>\"><%= local_assigns[:for] %></p>\n",
        ),
        ("app/views/posts/index.html.erb", "<%= render \"empty\", for: Post, class: \"muted\" %>\n"),
    ]);
    assert_parses(&files, "app/views/posts/_empty.rb");
    assert_parses(&files, "app/views/posts/index.rb");
}

/// B2, the repro shape: the same strict locals, read with
/// `binding.local_variable_get`.
#[test]
fn a_partial_declares_reserved_word_strict_locals_read_with_binding() {
    let files = posts_app(&[
        (
            "app/views/posts/_empty.html.erb",
            "<%# locals: (for:, class: \"\") %>\n<p class=\"<%= binding.local_variable_get(:class) %>\"><%= binding.local_variable_get(:for) %></p>\n",
        ),
        ("app/views/posts/index.html.erb", "<%= render \"empty\", for: Post, class: \"muted\" %>\n"),
    ]);
    assert_parses(&files, "app/views/posts/_empty.rb");
    assert_parses(&files, "app/views/posts/index.rb");
}

/// B3: a partial without strict locals reads `local_assigns[:class]`,
/// which became a positional `class`.
#[test]
fn a_partial_reads_a_reserved_word_local_with_local_assigns() {
    let files = posts_app(&[
        (
            "app/views/posts/_card.html.erb",
            "<div class=\"card <%= local_assigns[:class] %>\"><%= title %></div>\n",
        ),
        ("app/views/posts/index.html.erb", "<%= render \"card\", title: \"hi\", class: \"wide\" %>\n"),
    ]);
    assert_parses(&files, "app/views/posts/_card.rb");
    assert_parses(&files, "app/views/posts/index.rb");
}

/// `render partial:, collection:, as: :for` emitted a positional
/// `for` param in the partial and a `|for|` block param at the call.
#[test]
fn a_collection_render_binds_a_reserved_word_as_local() {
    let files = posts_app(&[
        (
            "app/views/posts/_row.html.erb",
            "<li><%= binding.local_variable_get(:for).title %></li>\n",
        ),
        (
            "app/views/posts/index.html.erb",
            "<%= render partial: \"row\", collection: @posts, as: :for %>\n",
        ),
    ]);
    assert_parses(&files, "app/views/posts/_row.rb");
    assert_parses(&files, "app/views/posts/index.rb");
}

/// A bang method on a reserved-word keyword stays a bang call. The
/// frozen-literal rewrite (`x.strip!` → `x = x.strip`) needs an
/// assignable local, and `class = …` does not parse.
#[test]
fn a_bang_method_on_a_reserved_word_keyword_stays_a_bang_call() {
    let files = posts_app(&[
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def trimmed(text, class: +\" a \")\n    binding.local_variable_get(:class).strip!\n    tag.i(text, class: binding.local_variable_get(:class))\n  end\nend\n",
        ),
        ("app/views/posts/index.html.erb", "<%= trimmed(\"hi\", class: +\" big \") %>\n"),
    ]);
    assert_parses(&files, "app/models/application_helper.rb");
}
