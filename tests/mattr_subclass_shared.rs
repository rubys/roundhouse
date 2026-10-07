//! Rails-faithful subclass-shared storage for `mattr_*` / `cattr_*`.
//!
//! ActiveSupport uses `@@` class variables: a subclass read sees the
//! declaring class's value, and a subclass write updates the parent.
//! `class_attribute` is the independent-per-subclass alternative (owned
//! elsewhere) — this suite pins only the shared mattr/cattr form.

use roundhouse::dialect::MethodReceiver;
use roundhouse::expr::{ExprNode, LValue};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::Symbol;
use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

#[test]
fn library_mattr_reader_uses_class_variable_storage() {
    let classes = roundhouse::ingest::ingest_library_classes(
        b"class Probe\n  mattr_accessor :channel\nend\n",
        "probe.rb",
    )
    .expect("ingest");
    let reader = classes[0]
        .methods
        .iter()
        .find(|m| m.name.as_str() == "channel" && m.receiver == MethodReceiver::Class)
        .expect("class reader");
    assert!(
        matches!(
            &*reader.body.node,
            ExprNode::Var { name, .. } if name.as_str() == "@@channel"
        ),
        "expected @@channel read, got {:?}",
        reader.body.node
    );
    assert!(
        classes[0].class_ivar_initializers.iter().any(|expr| {
            // Nil seeds are conditional: `@@attr = nil unless defined?`
            // so subclass redeclarations do not wipe the parent.
            matches!(
                &*expr.node,
                ExprNode::If {
                    else_branch,
                    ..
                } if matches!(
                    &*else_branch.node,
                    ExprNode::Assign {
                        target: LValue::Var { name, .. },
                        value,
                    } if name.as_str() == "@@channel"
                        && matches!(
                            &*value.node,
                            ExprNode::Lit { value: roundhouse::expr::Literal::Nil }
                        )
                )
            )
        }),
        "expected conditional @@channel = nil seed: {:?}",
        classes[0].class_ivar_initializers
    );
}

#[test]
fn model_mattr_and_cattr_seed_shared_classvars() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  mattr_accessor :channel\n  cattr_accessor :banner\nend\n",
        ),
    ]))
    .expect("ingest");
    let article = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Article")
        .expect("Article");
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("channel")));
    assert!(article.class_attr_defaults.contains_key(&Symbol::from("banner")));
    for name in ["channel", "banner"] {
        let reader = article
            .methods()
            .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Class)
            .unwrap_or_else(|| panic!("missing class reader {name}"));
        assert!(
            matches!(
                &*reader.body.node,
                ExprNode::Var { name: n, .. } if n.as_str() == format!("@@{name}")
            ),
            "{name}: {:?}",
            reader.body.node
        );
    }
}

#[test]
fn mattr_subclass_read_and_write_share_with_parent() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  mattr_accessor :channel
"#,
        )
        .write(
            "app/models/special_article.rb",
            "class SpecialArticle < Article\nend\n",
        )
        .run_ruby(
            r#"
raise "unset parent" unless Article.channel.nil?
raise "unset subclass" unless SpecialArticle.channel.nil?
Article.channel = "news"
raise "parent read" unless Article.channel == "news"
raise "subclass reads parent value" unless SpecialArticle.channel == "news"
SpecialArticle.channel = "sports"
raise "subclass write updates parent" unless Article.channel == "sports"
raise "subclass read after write" unless SpecialArticle.channel == "sports"
raise "instance reader shares" unless Article.new.channel == "sports"
Article.new.channel = "weather"
raise "instance writer updates class" unless SpecialArticle.channel == "weather"
puts "mattr_subclass_shared_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn cattr_subclass_read_and_write_share_with_parent() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  cattr_accessor :banner
"#,
        )
        .write(
            "app/models/special_article.rb",
            "class SpecialArticle < Article\nend\n",
        )
        .run_ruby(
            r#"
Article.banner = "hello"
raise "cattr parent read" unless Article.banner == "hello"
raise "cattr subclass read" unless SpecialArticle.banner == "hello"
SpecialArticle.banner = "world"
raise "cattr subclass write updates parent" unless Article.banner == "world"
puts "cattr_subclass_shared_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn library_mattr_and_cattr_share_across_subclass() {
    // Same shape as `class_variable_compound_writes_share_the_read_storage`:
    // a service class is loaded via the emitted app's constant lookup, not
    // `require_relative` (treeshake may place or omit paths differently).
    emit_and_run::real_blog()
        .write(
            "app/services/channel_config.rb",
            r#"class ChannelConfig
  mattr_accessor :channel
  cattr_accessor :banner
end
class SpecialChannelConfig < ChannelConfig
end
"#,
        )
        .run_ruby(
            r#"
ChannelConfig.channel = "news"
raise "lib mattr parent" unless ChannelConfig.channel == "news"
raise "lib mattr subclass" unless SpecialChannelConfig.channel == "news"
SpecialChannelConfig.channel = "sports"
raise "lib mattr write-through" unless ChannelConfig.channel == "sports"
ChannelConfig.banner = "hi"
raise "lib cattr parent" unless ChannelConfig.banner == "hi"
raise "lib cattr subclass" unless SpecialChannelConfig.banner == "hi"
SpecialChannelConfig.banner = "yo"
raise "lib cattr write-through" unless ChannelConfig.banner == "yo"
puts "library_mattr_cattr_subclass_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn subclass_redeclaration_preserves_inherited_value() {
    // Rails: redeclaring mattr/cattr (plain or default: nil) must not
    // wipe a value already set on the shared @@ storage.
    emit_and_run::real_blog()
        .write(
            "app/services/channel_config.rb",
            r#"class ChannelConfig
  mattr_accessor :channel
  cattr_accessor :banner
end
class SpecialChannelConfig < ChannelConfig
  mattr_accessor :channel
  cattr_accessor :banner, default: nil
end
"#,
        )
        .run_ruby(
            r#"
ChannelConfig.channel = "news"
ChannelConfig.banner = "hi"
raise "redeclare wiped mattr" unless SpecialChannelConfig.channel == "news"
raise "redeclare wiped cattr" unless SpecialChannelConfig.banner == "hi"
raise "parent mattr intact" unless ChannelConfig.channel == "news"
raise "parent cattr intact" unless ChannelConfig.banner == "hi"
puts "subclass_redeclaration_preserves_ok"
"#,
        )
        .assert_passes();
}

#[test]
fn library_mattr_default_seeds_shared_classvar() {
    let classes = roundhouse::ingest::ingest_library_classes(
        b"class Probe\n  mattr_accessor :channel, default: \"news\"\nend\n",
        "probe.rb",
    )
    .expect("ingest");
    assert!(
        classes[0].class_ivar_initializers.iter().any(|expr| {
            matches!(
                &*expr.node,
                ExprNode::Assign {
                    target: LValue::Var { name, .. },
                    value,
                } if name.as_str() == "@@channel"
                    && matches!(
                        &*value.node,
                        ExprNode::Lit {
                            value: roundhouse::expr::Literal::Str { value }
                        } if value == "news"
                    )
            )
        }),
        "expected @@channel = \"news\" seed: {:?}",
        classes[0].class_ivar_initializers
    );

    emit_and_run::real_blog()
        .write(
            "app/services/channel_config.rb",
            r#"class ChannelConfig
  mattr_accessor :channel, default: "news"
end
class SpecialChannelConfig < ChannelConfig
end
"#,
        )
        .run_ruby(
            r#"
raise "lib default parent" unless ChannelConfig.channel == "news"
raise "lib default subclass" unless SpecialChannelConfig.channel == "news"
SpecialChannelConfig.channel = "sports"
raise "lib default write-through" unless ChannelConfig.channel == "sports"
puts "library_mattr_default_ok"
"#,
        )
        .assert_passes();
}
