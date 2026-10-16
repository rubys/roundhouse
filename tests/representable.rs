//! `Representable::Decorator` - a bounded subset of the representable
//! gem, expanded before inference into plain methods
//! (`ingest::representable`), with its JSON writer settled after it
//! (`lower::as_json_poro`). The behavior against the gem's own output is
//! pinned in `emit_and_run.rs`; these pin the shape and the declines.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n    t.datetime :published_at\n  end\nend\n";

fn tree(representer: &str, controller: &str) -> HashMap<PathBuf, Vec<u8>> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/article.rb"),
        b"class Article < ApplicationRecord\n  def tags\n    [\"a\", \"b\"]\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/representers/article_representer.rb"), representer.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/controllers/articles_controller.rb"), controller.as_bytes().to_vec());
    tree
}

const CONTROLLER: &str = r#"class ArticlesController < ApplicationController
  def show
    article = Article.find(params[:id])
    render json: ArticleRepresenter.new(article).to_hash
  end
end
"#;

/// The emitted representer and controller, joined.
fn emitted(representer: &str) -> (String, String) {
    let mut app = ingest_app_from_tree(tree(representer, CONTROLLER)).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::target_files(
        &app,
        std::path::Path::new("fixtures/tiny-blog"),
        roundhouse::project::BuildTarget::Spinel,
    )
    .expect("emit");
    let find = |needle: &str| {
        files
            .iter()
            .filter(|(p, _)| p.ends_with(needle))
            .map(|(_, c)| c.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    (find("article_representer.rb"), find("articles_controller.rb"))
}

#[test]
fn the_decorator_becomes_a_plain_class_with_a_writer() {
    let (rep, ctrl) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :id\n  property :title\nend\n",
    );
    assert!(rep.contains("class ArticleRepresenter\n"), "no gem parent left:\n{rep}");
    assert!(!rep.contains("Representable"), "no gem constant left:\n{rep}");
    assert!(!rep.contains("property "), "the class-body DSL is not replayed:\n{rep}");
    assert!(rep.contains("def to_hash"), "{rep}");
    assert!(rep.contains("def as_json_str"), "{rep}");
    assert!(ctrl.contains("ArticleRepresenter.new(article).as_json_str"), "{ctrl}");
    assert!(!ctrl.contains("JsonRender"), "{ctrl}");
}

/// A value typed `Array[String]` is written by `encode_string_array`,
/// not quoted as one string by `encode_value`.
#[test]
fn a_string_array_value_takes_the_array_encoder() {
    let (rep, _) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :tags\nend\n",
    );
    assert!(rep.contains("JsonBuilder.encode_string_array(j_tags)"), "{rep}");
}

/// `published_at && published_at.iso8601` is `String?`: the left arm
/// only answers when it is falsy. Typed as `String | Time | nil`, the
/// Time would have no encoding and the writer would be dropped.
#[test]
fn a_guarded_getter_keeps_the_writer() {
    let (rep, ctrl) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :published_at, getter: ->(represented:, **) { represented.published_at&.iso8601 }\nend\n",
    );
    assert!(rep.contains("def as_json_str"), "{rep}");
    assert!(ctrl.contains(".as_json_str"), "{ctrl}");
}

/// A value with no JSON encoding here (a raw Time) drops the writer, and
/// the render site keeps the runtime encoder instead of a wrong text.
#[test]
fn an_unencodable_value_drops_the_writer() {
    let (rep, ctrl) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :published_at\nend\n",
    );
    assert!(rep.contains("def to_hash"), "{rep}");
    assert!(!rep.contains("def as_json_str"), "{rep}");
    assert!(ctrl.contains("JsonRender.encode"), "{ctrl}");
}

/// Strict: an option outside the subset fails ingest rather than being
/// lowered in part, and survey mode ledgers it.
#[test]
fn an_option_outside_the_subset_is_refused() {
    let representer = "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :title, if: ->(**) { true }\nend\n";
    assert!(ingest_app_from_tree(tree(representer, CONTROLLER)).is_err());
    survey::activate();
    let result = ingest_app_from_tree(tree(representer, CONTROLLER));
    let gaps = survey::drain();
    result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("Representable subset") && message.contains("`if:`"))),
        "{gaps:?}"
    );
}

/// `->(**) { title.upcase }` runs on the represented object, so its
/// receiverless call is rebased; a local and Kernel's `format` are not.
#[test]
fn a_represented_getter_is_rebased_onto_represented() {
    let (rep, _) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :title, getter: ->(**) { t = title.upcase\n format(\"%s!\", t) }\nend\n",
    );
    assert!(rep.contains("t = represented.title.upcase"), "{rep}");
    assert!(rep.contains("format(\"%s!\", t)"), "{rep}");
    assert!(!rep.contains("represented.format"), "{rep}");
    assert!(!rep.contains("represented.t)"), "{rep}");
}

/// A decorator method named like a generated one (`to_hash`, `initialize`,
/// `represented`, `as_json_str`, `representable_<name>`) would be replaced
/// by the generated method, and a class-side method is outside the subset:
/// both are refused rather than lowered in part.
#[test]
fn a_method_colliding_with_a_generated_one_is_refused() {
    for body in [
        "  def to_hash\n    {}\n  end\n",
        "  def initialize(model)\n    super\n  end\n",
        "  def representable_title\n    \"x\"\n  end\n",
        "  def self.build\n    new(nil)\n  end\n",
    ] {
        let representer = format!(
            "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :title\n\n{body}end\n"
        );
        assert!(ingest_app_from_tree(tree(&representer, CONTROLLER)).is_err(), "accepted:\n{representer}");
    }
}

/// `next` ends the getter only at its own level; one inside a nested block
/// ends that block's iteration and must stay `next`.
#[test]
fn only_a_top_level_next_becomes_return() {
    let (rep, _) = emitted(
        "class ArticleRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :title, getter: ->(**) { next \"none\" if title.nil?\n title.chars.map { |c| next \"_\" if c == \" \"\n c }.join }\nend\n",
    );
    assert!(rep.contains("return \"none\" if"), "{rep}");
    assert!(rep.contains("next \"_\" if"), "{rep}");
    assert!(!rep.contains("return \"_\""), "{rep}");
}
