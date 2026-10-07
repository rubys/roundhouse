//! Class-body `mattr_accessor` / `cattr_accessor` with documented
//! `default:` / block defaults, and ActiveRecord `serialize` JSON
//! coders — general Rails spellings, abstract overlays.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodReceiver, ModelBodyItem};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;
use roundhouse::Symbol;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn article_app(model_body: &str, schema_extra: &str) -> HashMap<PathBuf, Vec<u8>> {
    tree(&[
        (
            "db/schema.rb",
            &format!(
                "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n{schema_extra}  end\nend\n"
            ),
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/article.rb",
            &format!("class Article < ApplicationRecord\n{model_body}\nend\n"),
        ),
    ])
}

fn class_method_names(app: &roundhouse::App, model: &str) -> Vec<String> {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == model)
        .expect(model)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .map(|m| m.name.as_str().to_string())
        .collect()
}

#[test]
fn mattr_default_keyword_literal_shapes_expand() {
    let body = r#"
  mattr_accessor :label, default: "ok"
  mattr_accessor :count, default: 3
  mattr_accessor :flag, default: true
  mattr_accessor :empty, default: nil
  mattr_accessor :tags, default: []
  mattr_accessor :meta, default: {}
"#;
    let app = ingest_app_from_tree(article_app(body, "")).expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    for name in ["label", "count", "flag", "empty", "tags", "meta"] {
        assert!(
            article.class_attr_defaults.contains_key(&Symbol::from(name)),
            "missing default for {name}"
        );
        assert!(
            class_method_names(&app, "Article").contains(&name.to_string()),
            "missing class reader {name}: {:?}",
            class_method_names(&app, "Article")
        );
    }
}

#[test]
fn cattr_block_default_and_mattr_const_default_expand() {
    let body = r#"
  Probe = Object
  mattr_accessor :factory, default: Probe.new
  cattr_accessor :banner do
    "from_block"
  end
  cattr_reader :only_read, default: "r"
  mattr_writer :only_write, default: "w"
"#;
    let app = ingest_app_from_tree(article_app(body, "")).expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("factory")));
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("banner")));
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("only_read")));
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("only_write")));
    let names = class_method_names(&app, "Article");
    assert!(names.contains(&"factory".to_string()), "{names:?}");
    assert!(names.contains(&"banner".to_string()), "{names:?}");
    assert!(names.contains(&"only_read".to_string()), "{names:?}");
    assert!(names.contains(&"only_write=".to_string()), "{names:?}");
    assert!(
        !names.contains(&"only_write".to_string()),
        "mattr_writer must not add a reader"
    );
}

#[test]
fn model_mattr_defaults_keep_partitioned_emit() {
    // Source-ordered class bodies interleave by span.start. Synthetic
    // mattr seeds sit at span 0; if models took that path, `Probe.new`
    // would emit before `Probe = Object`. Keep models on the partitioned
    // constants-then-seeds path.
    let (emitted, errors) = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  Probe = Object\n  mattr_accessor :factory, default: Probe.new\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "emit errors: {errors:?}");
    let article = std::fs::read_to_string(emitted.join("app/models/article.rb"))
        .expect("read emitted article");
    let probe = article
        .find("Probe = Object")
        .expect("Probe constant missing from emit");
    let factory = article
        .find("@@factory")
        .expect("@@factory seed missing from emit");
    assert!(
        probe < factory,
        "Probe constant must precede @@factory seed:\n{article}"
    );
}

#[test]
fn instance_reader_kwarg_stays_unexpanded() {
    let body = "  mattr_accessor :hidden, instance_reader: false, default: 1\n";
    let app = ingest_app_from_tree(article_app(body, "")).expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .unwrap();
    assert!(article.class_attr_defaults.is_empty());
    assert!(article.body.iter().any(|item| matches!(
        item,
        ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "mattr_accessor")
    )));
}

#[test]
fn serialize_json_coder_variants_are_claimed() {
    for decl in [
        "serialize :payload, coder: JSON",
        "serialize :payload, JSON",
        "serialize :payload, coder: ::JSON",
    ] {
        let body = format!("  {decl}\n");
        let app = ingest_app_from_tree(article_app(
            &body,
            "    t.text :payload\n",
        ))
        .expect("ingest");
        let article = app
            .models
            .iter()
            .find(|m| m.name.0.as_str() == "Article")
            .unwrap();
        assert!(
            article.body.iter().any(|item| matches!(
                item,
                ModelBodyItem::Unknown { expr, .. }
                    if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "serialize")
            )),
            "serialize stays Unknown until lower claims it: {decl}"
        );
        let decls = roundhouse::lower::serialize::serialize_decls(article);
        assert_eq!(decls.len(), 1, "{decl}");
        assert_eq!(decls[0].column.as_str(), "payload");
    }
}

#[test]
fn serialize_yaml_default_stays_unclaimed() {
    let app = ingest_app_from_tree(article_app(
        "  serialize :payload\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert!(roundhouse::lower::serialize::serialize_decls(article).is_empty());
}

#[test]
fn serialize_json_shadowed_by_local_const_stays_unclaimed() {
    let app = ingest_app_from_tree(article_app(
        "  JSON = Object\n  serialize :payload, coder: JSON\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert!(
        roundhouse::lower::serialize::serialize_decls(article).is_empty(),
        "bare JSON shadowed by a model constant must not claim JsonColumn"
    );
    // Absolute ::JSON still names the stdlib coder.
    let app = ingest_app_from_tree(article_app(
        "  JSON = Object\n  serialize :payload, coder: ::JSON\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(
        roundhouse::lower::serialize::serialize_decls(article).len(),
        1
    );
}

#[test]
fn serialize_json_string_assignment_round_trips() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.text \"payload\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  serialize :payload, coder: JSON\n",
        )
        .run_ruby(
            r#"
a = Article.create!(title: "S", body: "A body long enough to validate.", payload: "true")
raise "assigned string" unless Article.find(a.id).payload == "true"
a.payload = { "k" => 1 }
a.save!
raise "hash after string" unless Article.find(a.id).payload == { "k" => 1 }
puts "serialize_string_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn serialize_json_shadow_detection_is_scope_aware() {
    // Unrelated Foo::JSON must not unclaim bare coder: JSON.
    let app = ingest_app_from_tree(article_app(
        "  Foo::JSON = Object\n  serialize :payload, coder: JSON\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(
        roundhouse::lower::serialize::serialize_decls(article).len(),
        1,
        "Foo::JSON must not shadow bare JSON on Article"
    );

    // Owner-qualified Article::JSON does shadow.
    let app = ingest_app_from_tree(article_app(
        "  Article::JSON = Object\n  serialize :payload, coder: JSON\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert!(
        roundhouse::lower::serialize::serialize_decls(article).is_empty(),
        "Article::JSON shadows bare JSON"
    );

    // OpAssign JSON ||= …
    let app = ingest_app_from_tree(article_app(
        "  JSON ||= Object\n  serialize :payload, coder: JSON\n",
        "    t.text :payload\n",
    ))
    .expect("ingest");
    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert!(
        roundhouse::lower::serialize::serialize_decls(article).is_empty(),
        "JSON ||= shadows bare JSON"
    );

    // Nested class JSON / module JSON leave a shadow marker.
    for nested in ["class JSON; end", "module JSON; end"] {
        let app = ingest_app_from_tree(article_app(
            &format!("  {nested}\n  serialize :payload, coder: JSON\n"),
            "    t.text :payload\n",
        ))
        .expect("ingest");
        let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
        assert!(
            roundhouse::lower::serialize::serialize_decls(article).is_empty(),
            "nested `{nested}` shadows bare JSON"
        );
    }
}

#[test]
fn serialize_json_enclosing_module_const_stays_unclaimed() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n    t.text :payload\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/article.rb",
            r#"module Admin
  JSON = Object
  class Article < ApplicationRecord
    serialize :payload, coder: JSON
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Admin::Article")
        .expect("Admin::Article");
    assert!(
        article.lexical_json_shadow,
        "Admin::JSON must set lexical_json_shadow"
    );
    assert!(
        roundhouse::lower::serialize::serialize_decls(article).is_empty(),
        "enclosing Admin::JSON must not claim bare coder: JSON"
    );
    // Absolute ::JSON still claims under the same nest.
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n    t.text :payload\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/article.rb",
            r#"module Admin
  JSON = Object
  class Article < ApplicationRecord
    serialize :payload, coder: ::JSON
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Admin::Article")
        .unwrap();
    assert_eq!(
        roundhouse::lower::serialize::serialize_decls(article).len(),
        1
    );
}

#[test]
fn class_body_mattr_and_cattr_defaults_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  mattr_accessor :channel, default: "news"
  mattr_accessor :attempts, default: 2
  cattr_accessor :banner do
    "hello"
  end
  mattr_accessor :probe, default: Object.new
  # Multi-stmt block default — assign value is the last expression.
  cattr_accessor :composed do
    inner = "INNER"
    "OUTER-#{inner}"
  end
"#,
        )
        .run_ruby(
            r#"
raise "mattr str" unless Article.channel == "news"
raise "mattr int" unless Article.attempts == 2
raise "cattr block" unless Article.banner == "hello"
raise "instance shares class seed" unless Article.new.channel == "news"
Article.channel = "sports"
raise "writer" unless Article.channel == "sports"
raise "probe class" unless Article.probe.is_a?(Object)
raise "multi-stmt block" unless Article.composed == "OUTER-INNER"
puts "mattr_cattr_defaults_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn serialize_json_coder_round_trips() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.text \"payload\"\n    t.text \"legacy\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  serialize :payload, coder: JSON
  serialize :legacy, JSON
"#,
        )
        .write(
            "test/models/article_serialize_json_test.rb",
            r#"require "test_helper"

class ArticleSerializeJsonTest < ActiveSupport::TestCase
  test "coder JSON and positional JSON round-trip decoded values" do
    value = [{ "name" => "Ada", "ok" => true }]
    article = Article.create!(title: "S", body: "A body long enough to validate.", payload: value, legacy: { "a" => 1 })
    article = Article.find(article.id)
    assert_equal value, article.payload
    assert_equal({ "a" => 1 }, article.legacy)
    article.payload = { "name" => "Grace" }
    article.save!
    assert_equal({ "name" => "Grace" }, Article.find(article.id).payload)
  end
end
"#,
        )
        .run_test("test/models/article_serialize_json_test.rb")
        .assert_passes();
}
