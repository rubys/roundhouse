//! `render json: @stories` where the model writes its own `as_json` —
//! lobsters' `/hottest` and every other story listing.
//!
//! Two halves have to meet. The model's `as_json` is read as an ordered
//! pair list (`lower::as_json_shape`) and, when analysis types every
//! value, written down as an `as_json_str` writer
//! (`lower::as_json_poro` → `as_json_writer::typed_writer_method`). And
//! the render site's value has to be TYPED as a collection of that
//! model, which in lobsters means following it through the pagination
//! helpers: `@stories, @show_more = get_from_cache { paginate … }`,
//! where `get_from_cache` answers its block's value and `paginate`
//! answers `[stories, show_more]`.
//!
//! The miniature below keeps each link of that chain. The emitted text
//! is what is asserted; that the text is Rails' JSON was checked
//! against the runtime encoder on the real app (byte-identical, 25
//! stories).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do
  create_table :stories do |t|
    t.string :title
    t.integer :comments_count
    t.datetime :created_at
  end
  create_table :tags do |t|
    t.string :tag, null: false
    t.integer :story_id
  end
end
";

const STORY: &str = r#"class Story < ApplicationRecord
  has_many :tags

  def short_id_url
    "http://example.com/s/#{id}"
  end

  def as_json(options = {})
    keys = [
      :title,
      :created_at,
      {comment_count: :comments_count},
      {tags: tags.map(&:tag).sort}
    ]
    if options && options[:with_comments]
      keys.push(comments: options[:with_comments])
    end

    json = {}
    keys.each do |k|
      if k.is_a?(Symbol)
        json[k] = send(k)
      elsif k.is_a?(Hash)
        json[k.keys.first] = if k.values.first.is_a?(Symbol)
          send(k.values.first)
        else
          k.values.first
        end
      end
    end

    json[:short_id_url] = short_id_url

    json
  end
end
"#;

const TAG: &str = "class Tag < ApplicationRecord\n  belongs_to :story\nend\n";

const PAGINATOR: &str = r#"class StoriesPaginator
  def initialize(scope, page = 1)
    @scope = scope
    @page = page
  end

  def get
    with_pagination_info @scope.limit(26).offset((@page - 1) * 25)
  end

  private

  def with_pagination_info(scope)
    scope = scope.to_a
    show_more = scope.count > 25
    [scope, show_more]
  end
end
"#;

const CONTROLLER: &str = r#"class StoriesController < ApplicationController
  def index
    @stories, @show_more = get_from_cache(hottest: true) {
      paginate Story.all
    }

    respond_to do |format|
      format.html { render action: "index" }
      format.json { render json: @stories }
    end
  end

  private

  def paginate(scope)
    StoriesPaginator.new(scope, 1).get
  end

  def get_from_cache(opts = {}, &)
    if @user
      yield
    else
      Rails.cache.fetch("stories", expires_in: 45, &)
    end
  end
end
"#;

fn analyzed(story: &str) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/story.rb"), story.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/tag.rb"), TAG.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/stories_paginator.rb"), PAGINATOR.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/stories_controller.rb"),
        CONTROLLER.as_bytes().to_vec(),
    );
    tree.insert(
        PathBuf::from("app/views/stories/index.html.erb"),
        b"<% @stories.each do |s| %><%= s.title %><% end %>\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :stories, only: [:index]\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn file(files: Vec<roundhouse::emit::EmittedFile>, needle: &str) -> String {
    files
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains(needle))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The spinel-shape controller: its json arm used to be DROPPED (inline
/// `render json:` needs an encoder that tree has none of). Typed, the
/// site serves the writer's text and the arm survives.
#[test]
fn the_json_arm_serves_each_records_writer() {
    let app = analyzed(STORY);
    let src = file(ruby::emit_lowered_controllers(&app), "stories_controller");
    assert!(src.contains("request_format == :json"), "the json arm was dropped:\n{src}");
    assert!(
        src.contains(r#""[" + @stories.map { |item| item.as_json_str }.join(",") + "]""#),
        "{src}"
    );
    assert!(src.contains(r#"content_type: "application/json""#), "{src}");
    assert!(!src.contains("JsonRender"), "{src}");
}

/// Each value encodes by its analyzed type: a temporal column in the
/// app's zone, an `Array[String]` as an array, a scalar as a scalar —
/// and the post-hoc `json[:k] = v` after the walk is a key too.
#[test]
fn the_writer_encodes_each_pair_by_its_type() {
    let app = analyzed(STORY);
    let src = file(ruby::emit_lowered_models(&app), "models/story.rb");
    let writer = &src[src.find("def as_json_str").expect(&src)..];
    let order = [
        r#""{""#,
        r#""\"title\":""#,
        "JsonBuilder.encode_value(self.title)",
        r#"",\"created_at\":""#,
        "JsonBuilder.encode_value(ActiveSupport.json_time(self.created_at_raw))",
        r#"",\"comment_count\":""#,
        "JsonBuilder.encode_value(self.comments_count)",
        r#"",\"tags\":""#,
        "JsonBuilder.encode_string_array(",
        r#"",\"short_id_url\":""#,
        r#""}""#,
    ];
    let mut at = 0;
    for piece in order {
        let found = writer[at..].find(piece).unwrap_or_else(|| panic!("missing {piece} after {at}:\n{writer}"));
        at += found + piece.len();
    }
    // The `with_comments` push is unreachable for the bare call `render
    // json:` makes, so no `comments` key.
    assert!(!writer[..at].contains("comments\\\":"), "{writer}");
}

/// A value whose type has no encoding here (a nested record) declines
/// the WHOLE model: no writer, and the site keeps the runtime encoder —
/// never a quoted `to_s` of the record.
#[test]
fn an_unencodable_value_keeps_the_runtime_encoder() {
    let story = STORY.replace("{tags: tags.map(&:tag).sort}", "{first_tag: tags.first}");
    let app = analyzed(&story);
    let models = file(ruby::emit_lowered_models(&app), "models/story.rb");
    assert!(!models.contains("as_json_str"), "{models}");
    let src = file(ruby::emit_lowered_controllers_with_layout(&app), "stories_controller");
    assert!(src.contains("ActionController::JsonRender.encode(@stories)"), "{src}");
}

/// A table-backed record with NO `as_json` serializes its columns in
/// Rails, not the declared readers a tableless class's writer reads —
/// here `Tag`, whose `attr_accessor` is no part of its payload.
#[test]
fn a_record_without_its_own_as_json_writes_its_columns() {
    let tag = "class Tag < ApplicationRecord\n  belongs_to :story\n  attr_accessor :filtered_count\nend\n";
    let controller = r#"class StoriesController < ApplicationController
  def index
    @tags = Tag.all
    render json: @tags
  end
end
"#;
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/story.rb"), b"class Story < ApplicationRecord\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/tag.rb"), tag.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/controllers/stories_controller.rb"), controller.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :stories, only: [:index]\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let models = file(ruby::emit_lowered_models(&app), "models/tag.rb");
    let writer = &models[models.find("def as_json_str").unwrap_or_else(|| panic!("{models}"))..];
    let writer = &writer[..writer.find("\n  end\n").unwrap_or(writer.len())];
    assert!(writer.contains(r#"\"id\":"#) && writer.contains(r#",\"tag\":"#) && writer.contains(r#",\"story_id\":"#), "{writer}");
    assert!(!writer.contains("filtered_count"), "{writer}");
    let src = file(ruby::emit_lowered_controllers_with_layout(&app), "stories_controller");
    assert!(src.contains("as_json_str") && !src.contains("JsonRender"), "{src}");
}
