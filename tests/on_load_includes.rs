//! Load-hook mixin installation is a reported gap, not Markdown support.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ModelBodyItem;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey};

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(hooks: &str) -> HashMap<PathBuf, Vec<u8>> {
    [
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("lib/rails_ext/hooks.rb", hooks),
        ("app/models/page.rb", "class Page < ApplicationRecord\n  has_markdown :body\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :pages do |t|\n    t.string :title\n  end\n  create_table :action_text_markdowns do |t|\n    t.string :record_type\n    t.integer :record_id\n    t.string :name\n    t.text :content\n  end\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect()
}

#[test]
fn direct_includes_name_the_hook_and_source_without_installing_anything() {
    // First hook is Writebook's installer, verbatim. Later hooks prove
    // that a literal-name-only recognizer cannot silently skip other shapes.
    let files = tree(
        r#"ActiveSupport.on_load :active_record do
  include ActionText::HasMarkdown
end
ActiveSupport.on_load(:action_text_markdown) do
  include First, Second
  include choose_mixin("märgistus")
end
"#,
    );
    let mut strict = ingest_app_from_tree(files.clone()).expect("strict ingest");
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let mut surveyed = result.expect("survey ingest");
    let messages: Vec<_> = gaps.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), 3, "one gap per include: {messages:?}");
    for (hook, include) in [
        ("active_record", "include ActionText::HasMarkdown"),
        ("action_text_markdown", "include First, Second"),
        (
            "action_text_markdown",
            "include choose_mixin(\"märgistus\")",
        ),
    ] {
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.contains("lib/rails_ext/hooks.rb")
                    && m.contains(&format!("on_load(:{hook})"))
                    && m.contains(&format!("`{include}`"))
                    && m.contains("load-hook mixin installation is unsupported"))
                .count(),
            1
        );
    }
    assert_eq!(surveyed, strict, "reporting must not change ingested IR");
    let page = surveyed
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Page")
        .unwrap();
    assert!(page.body.iter().any(|item| matches!(item,
        ModelBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
            ExprNode::Send { method, .. } if method.as_str() == "has_markdown"))));
    assert!(!surveyed.models.iter().any(|m| matches!(
        m.name.0.as_str(),
        "ActionText::Markdown" | "ActionText::RichText"
    )));
    let strict_diags = roundhouse::session::analyze_and_lower(&mut strict);
    let survey_diags = roundhouse::session::analyze_and_lower(&mut surveyed);
    assert_eq!(
        survey_diags, strict_diags,
        "no analysis/lowering support added"
    );
    let (strict_files, strict_emit) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_models(&strict)
    });
    let (survey_files, survey_emit) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_models(&surveyed)
    });
    assert_eq!(survey_files, strict_files, "no emitted behavior changed");
    assert_eq!(survey_emit, strict_emit);
    assert!(
        survey_emit
            .iter()
            .any(|d| d.message.contains("has_markdown")),
        "declaration warning must remain: {survey_emit:?}"
    );
    let page = survey_files
        .iter()
        .find(|f| f.path.ends_with("page.rb"))
        .unwrap();
    for method in [
        "body",
        "body?",
        "body=",
        "markdown_body",
        "build_markdown_body",
        "with_markdown_body",
        "with_markdown_body_and_embeds",
    ] {
        assert!(
            !page.content.lines().any(|line| line
                .trim_start()
                .starts_with(&format!("def {method}("))
                || line.trim() == format!("def {method}")),
            "not supported: {method}\n{}",
            page.content
        );
    }
}

#[test]
fn only_direct_receiverless_includes_in_top_level_active_support_hooks_are_reported() {
    survey::activate();
    let result = ingest_app_from_tree(tree(
        r#"ActiveSupport.on_load(:active_record) do
  self.include ExplicitReceiver
  Other.include OtherReceiver
  if enabled?
    include Conditional
  end
  def deferred
    include MethodBody
  end
  class Reopened
    include ClassBody
    def marker; 1; end
  end
end
Other.on_load(:active_record) do
  include NotActiveSupport
end
register do
  ActiveSupport.on_load(:active_record) do
    include NestedHook
  end
end
include NotInHook
"#,
    ));
    let gaps = survey::drain();
    result.expect("ingest controls");
    let messages: Vec<_> = gaps.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), 1, "only existing reopen gap: {messages:?}");
    assert!(messages[0].contains("reopens `Reopened`"));
    assert!(!messages[0].contains("mixin installation"));
}

#[test]
fn binary_encoded_source_uses_original_byte_locations_before_lossy_display() {
    let mut files = tree("");
    // An invalid UTF-8 byte before the include shifts lossy-string
    // offsets; one inside the argument can put an endpoint inside U+FFFD.
    files.insert(PathBuf::from("lib/rails_ext/hooks.rb"),
        b"# encoding: ASCII-8BIT\n# \xff\nActiveSupport.on_load(:active_record) do\n  include First\n  include \"\xff\"\nend\n".to_vec());
    let strict = ingest_app_from_tree(files.clone()).expect("strict binary-source ingest");
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let surveyed = result.expect("survey binary-source ingest");
    assert_eq!(surveyed, strict, "a ledger cannot change binary-source IR");
    let messages: Vec<_> = gaps.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    for (message, declaration) in messages.iter().zip(["include First", "include \"�\""]) {
        assert!(message.contains(&format!("`{declaration}`")), "{message}");
        assert!(message.contains("on_load(:active_record)"), "{message}");
        assert!(message.contains("lib/rails_ext/hooks.rb"), "{message}");
    }
}

#[test]
fn emitted_app_does_not_gain_methods_from_a_dropped_installer_or_string_class_eval() {
    // This is a negative execution check. The provider is a minimal
    // string-eval macro, not a replacement implementation of Writebook.
    let run = emit_and_run::real_blog()
        .write("lib/rails_ext/action_text_has_markdown.rb", r#"module ActionText::HasMarkdown
  extend ActiveSupport::Concern
  class_methods do
    def has_markdown(name)
      class_eval "def body; 99; end"
    end
  end
  def markdown_installer_marker
    37
  end
end
ActiveSupport.on_load :active_record do
  include ActionText::HasMarkdown
end
"#)
        .write("app/models/page.rb", "class Page < ApplicationRecord\n  has_markdown :body\nend\n")
        .edit("db/schema.rb", "  add_foreign_key \"comments\", \"articles\"", "  create_table :pages do |t|\n    t.string :title\n  end\n  add_foreign_key \"comments\", \"articles\"")
        .run_ruby(r#"
page = Page.new
[:body, :body?, :body=, :markdown_body, :build_markdown_body].each do |name|
  raise "invented Markdown method #{name}" if page.respond_to?(name, true)
end
[page, Article.new].each do |owner|
  raise "dropped hook installed mixin on #{owner.class}" if owner.respond_to?(:markdown_installer_marker, true)
end
[:with_markdown_body, :with_markdown_body_and_embeds].each do |name|
  raise "invented preload scope #{name}" if Page.respond_to?(name, true)
end
begin
  Page.new.body
  raise "string class_eval was expanded"
rescue NoMethodError => e
  raise "wrong missing method" unless e.name == :body
end
puts "unsupported Markdown boundary preserved"
"#);
    run.assert_passes();
    assert!(
        run.stdout
            .contains("unsupported Markdown boundary preserved")
    );
}
