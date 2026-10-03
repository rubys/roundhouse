//! Model-DSL MACROS declared inside a concern's `included do` reach the
//! models that include it.
//!
//! `has_many` and friends already did — the classifier gives them a
//! `ModelBodyItem` variant and the splice carries every one. The macros
//! that never got a variant (`has_one_attached`, `has_rich_text`,
//! `has_secure_token`, …) land in the `Unknown` holding pen instead,
//! and the splice kept only the block-form callbacks out of it. So
//! campfire's `Message::Attachment`, whose entire `included do` is
//! `has_one_attached :attachment`, contributed nothing: `Message#
//! attachment` was never synthesized, and the concern's own
//! `attachment?` — which calls it — emitted right beside the hole.
//!
//! The BLOCK form is the one campfire writes (`do |attachable|
//! attachable.variant :thumb, … end`). Variants aren't modeled; the
//! attachment-existence half still has to expand.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#,
        ),
        (
            "app/models/message/attachment.rb",
            r#"module Message::Attachment
  extend ActiveSupport::Concern

  THUMBNAIL_MAX_WIDTH = 1200

  included do
    has_one_attached :attachment do |attachable|
      attachable.variant :thumb, resize_to_limit: [ THUMBNAIL_MAX_WIDTH, 800 ]
    end
  end

  def attachment?
    attachment.attached?
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Attachment
end
"#,
        ),
    ]))
    .expect("ingest concern-macro app")
}

fn model_src(name: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

/// Rails' macro declares `with_attached_<attr>` beside the attachment,
/// and campfire's `Message.ordered` chains through it — so leaving it
/// undefined was a NameError on every room page, not a slow query. It
/// preloads Rails' own association name for the attachment, so the
/// relation's `to_a` batches every record's row into one query.
#[test]
fn the_attachment_preload_scope_exists_and_preloads_the_attachment() {
    let app = app();
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Message").unwrap();
    let names: Vec<String> = roundhouse::lower::attached::preload_scope_names(model)
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    assert_eq!(names, vec!["with_attached_attachment"]);

    let src = model_src("message.rb");
    assert!(
        src.contains("def self.with_attached_attachment"),
        "the preload scope must have a body:\n{src}"
    );
    let body = src
        .split("def self.with_attached_attachment")
        .nth(1)
        .and_then(|s| s.split("\n  end").next())
        .unwrap_or_default();
    assert!(
        body.contains("__rel.preload(:attachment_attachment)"),
        "the scope must preload the attachment association:\n{body}"
    );
}

/// One proxy per record, as in Rails: the reader remembers the
/// `Attached` it built, and the batch loader has a setter to install a
/// row-bearing one.
#[test]
fn the_attachment_reader_memoizes_its_proxy() {
    let src = model_src("message.rb");
    let at = src.find("def attachment\n").unwrap_or_else(|| panic!("{src}"));
    let body = &src[at..src[at..].find("\n  end").map(|i| at + i).unwrap_or(src.len())];
    assert!(body.contains("@attachment_cache"), "the reader keeps the proxy:\n{body}");
    assert!(
        body.contains(r#"ActiveStorage::Attached.new("Message", @id, "attachment", ["#),
        "and builds it on the first read:\n{body}"
    );
    assert!(
        src.contains("def _preload_attachment_attachment(att)"),
        "the batch loader's setter exists:\n{src}"
    );
}

/// The reader the concern declares lands on the INCLUDER, scoped to the
/// includer's own record type — not on the module, which has no table.
/// The block's `attachable.variant :thumb, resize_to_limit: [W, H]`
/// rides into the constructor as a `Variation`, with the concern's
/// constant spelled as the source spells it: inside `Message`, which
/// includes the concern, the bare name resolves.
#[test]
fn a_concerns_has_one_attached_reaches_the_includer() {
    let src = model_src("message.rb");
    assert!(
        src.contains(
            r#"ActiveStorage::Attached.new("Message", @id, "attachment", [ActiveStorage::Variation.new("thumb", THUMBNAIL_MAX_WIDTH, 800, "")])"#
        ),
        "the concern's has_one_attached must synthesize on Message with its variant:\n{src}"
    );
}

#[test]
fn literal_accessors_keep_their_source_and_lower_on_each_direct_includer() {
    use roundhouse::dialect::{ModelBodyItem, MethodReceiver};
    use roundhouse::expr::{ExprNode, Literal};

    let source = "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch, :flag\n  end\nend\n";
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :notes do |t|\n    t.string :body\n  end\nend\n"),
        ("app/models/concerns/draft_state.rb", source),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  include DraftState\nend\n"),
        ("app/models/other.rb", "class Other < ApplicationRecord\nend\n"),
    ])).expect("ingest literal concern accessors");
    let declaration = &app.concern_model_items.values().next().unwrap()[0];
    let ModelBodyItem::Unknown { expr: original, .. } = declaration else { panic!("{declaration:?}") };
    assert_eq!(original.span.start as usize, source.find("attr_accessor").unwrap());
    assert_eq!(original.span.end as usize, source.find(":flag").unwrap() + ":flag".len());

    for name in ["Message", "Note", "Other"] {
        let model = app.models.iter().find(|m| m.name.0.as_str() == name).unwrap();
        let carried = model.body.iter().filter_map(|item| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            let ExprNode::Send { method, .. } = &*expr.node else { return None };
            (method.as_str() == "attr_accessor").then_some(expr)
        }).collect::<Vec<_>>();
        assert_eq!(carried.len(), usize::from(name != "Other"), "{name}");
        if let Some(expr) = carried.first() {
            assert_eq!(expr.span, original.span);
            let ExprNode::Send { recv: None, args, block: None, .. } = &*expr.node else { panic!("{expr:?}") };
            let names = args.iter().map(|arg| {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node else { panic!("{arg:?}") };
                value.as_str()
            }).collect::<Vec<_>>();
            assert_eq!(names, ["scratch", "flag"]);
        }
        assert!(model.attributes.fields.iter().all(|(n, _)| !["scratch", "flag"].contains(&n.as_str())));
        let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
        for method_name in ["scratch", "scratch=", "flag", "flag="] {
            let methods = lowered.methods.iter().filter(|m| m.name.as_str() == method_name).collect::<Vec<_>>();
            assert_eq!(methods.len(), usize::from(name != "Other"), "{name}#{method_name}");
            if let Some(method) = methods.first() {
                assert_eq!(method.receiver, MethodReceiver::Instance);
                assert_eq!(method.enclosing_class.as_ref(), Some(&model.name.0));
            }
        }
    }
}

/// Unsupported shapes are contextual refusals, not executable accessors
/// or global errors for dormant concerns. Survey must record each includer
/// and remove the entire declaration, including its otherwise-valid names.
#[test]
fn unsupported_accessor_shapes_are_reported_only_for_includers() {
    use roundhouse::dialect::ModelBodyItem;
    use roundhouse::ingest::survey;

    for declaration in [
        "attr_accessor",
        "attr_accessor \"scratch\"",
        "attr_accessor :scratch, \"flag\"",
        "attr_accessor name",
        "attr_accessor :scratch, name",
        "attr_accessor *FIELDS",
        "attr_accessor :scratch, *FIELDS",
        "attr_accessor(:scratch) { nil }",
        "attr_reader :scratch",
        "attr_writer :scratch",
        "attr_accessor :scratch if false",
        "attr_accessor :scratch if true",
        "attr_reader :scratch unless false",
        "if false\n      attr_accessor :scratch\n    else\n      attr_writer :flag\n    end",
    ] {
        let source = format!("module DraftState\n  extend ActiveSupport::Concern\n  FIELDS = %i[scratch flag]\n  included do\n    {declaration}\n  end\nend\n");
        let (items, enums) = roundhouse::ingest::library_class::ingest_concern_model_items(source.as_bytes(), "draft_state.rb");
        assert_eq!(items.len(), 1, "recognized declaration must be retained: {declaration}");
        let [ModelBodyItem::Unknown { expr, .. }] = items[0].1.as_slice() else { panic!("{items:?}") };
        assert!(expr.diagnostic.is_some(), "unsupported declaration needs a contextual refusal: {declaration}");
        assert!(enums.is_empty());

        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :notes do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", source.as_str()),
            ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
        ];
        ingest_app_from_tree(tree(&files)).expect("dormant unsupported accessor must remain inert");
        let mut included = tree(&files);
        included.insert("app/models/message.rb".into(), b"class Message < ApplicationRecord\n  include DraftState\nend\n".to_vec());
        included.insert("app/models/note.rb".into(), b"class Note < ApplicationRecord\n  include DraftState\nend\n".to_vec());
        let error = ingest_app_from_tree(included.clone()).expect_err("unsupported included accessor must fail strict ingest");
        assert!(error.to_string().contains("unsupported accessor shape"), "{declaration}: {error}");
        survey::activate();
        let result = ingest_app_from_tree(included);
        let gaps = survey::drain();
        let app = result.expect("survey retains both includers");
        assert_eq!(gaps.len(), 2, "one refusal per includer: {declaration}: {gaps:?}");
        let declaration_span = app.concern_model_items.values().flatten().find_map(|item| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            expr.diagnostic.as_ref().map(|_| expr.span)
        }).expect("original refused declaration retains its source span");
        for name in ["Message", "Note"] {
            assert!(gaps.iter().any(|gap| gap.to_string().contains(&format!("on {name}"))), "{gaps:?}");
            let model = app.models.iter().find(|model| model.name.0.as_str() == name).unwrap();
            assert!(model.body.iter().all(|item| !matches!(item, ModelBodyItem::Unknown { expr: carried, .. } if carried.span == declaration_span)));
            let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
            assert!(lowered.methods.iter().all(|method| !["scratch", "scratch=", "flag", "flag="].contains(&method.name.as_str())), "must not partially synthesize {declaration}");
        }
    }
}

/// Receiver-bearing calls and unrelated DSL keep their existing path;
/// recognizing attr_* must not start treating arbitrary calls as macros.
#[test]
fn unrelated_calls_are_not_accessor_candidates() {
    for declaration in ["self.attr_accessor :scratch", "Other.attr_accessor :scratch", "unknown_macro :scratch"] {
        let source = format!("module DraftState\n  extend ActiveSupport::Concern\n  included {{ {declaration} }}\nend\n");
        let (items, enums) = roundhouse::ingest::library_class::ingest_concern_model_items(source.as_bytes(), "draft_state.rb");
        assert!(items.is_empty(), "must not claim unrelated {declaration}: {items:?}");
        assert!(enums.is_empty());
    }
}

/// Newly accepted accessors must be rejected, not silently publicized
/// or used to replace a different generated storage contract.
#[test]
fn accessor_contexts_fail_strict_ingest_and_are_not_carried_in_survey() {
    use roundhouse::dialect::ModelBodyItem;
    use roundhouse::expr::ExprNode;
    use roundhouse::ingest::survey;

    for (included_body, model_body) in [
        ("attr_accessor :scratch", "include DraftState\n  private def scratch\n    'private override'\n  end"),
        ("attr_accessor :scratch", "include DraftState\n  protected def scratch\n    'protected override'\n  end"),
        ("attr_accessor :scratch", "include DraftState\n  private def scratch=(value)\n    @scratch = value\n  end"),
        ("private\n    attr_accessor :scratch", "include DraftState"),
        ("protected\n    attr_accessor :scratch", "include DraftState"),
        ("attr_accessor :scratch\n    private :scratch", "include DraftState"),
        ("with_options if: false do\n      attr_accessor :scratch\n    end", "include DraftState"),
        ("attr_accessor :scratch\n    def scratch\n      'included override'\n    end", "include DraftState"),
        ("attr_accessor :scratch", "def scratch\n    'earlier'\n  end\n  include DraftState"),
        ("attr_accessor :scratch", "def scratch=(value)\n    @scratch = value\n  end\n  include DraftState"),
        ("attr_accessor :body", "include DraftState"),
        ("attr_accessor :scratch, :body", "include DraftState"),
        ("attr_accessor :body_was", "include DraftState"),
        ("attr_accessor :table_name", "include DraftState"),
        ("attr_accessor :comments", "include DraftState\n  has_many :comments"),
        ("attr_accessor :__comments_cache", "include DraftState\n  has_many :comments"),
        ("attr_accessor :_autosave_message", "include DraftState\n  belongs_to :message"),
        ("attr_accessor :persisted", "include DraftState"),
        ("attr_accessor :destroyed", "include DraftState"),
        ("attr_accessor :id_previously_changed", "include DraftState"),
        ("attr_accessor :scratch", "include DraftState\n  private\n  def scratch\n    'private override'\n  end"),
        ("attr_accessor :scratch", "primary_abstract_class\n  include DraftState"),
        ("attr_accessor :scratch", "self.abstract_class = true\n  include DraftState"),
    ] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    {included_body}\n  end\nend\n");
        let model = format!("class Message < ApplicationRecord\n  {model_body}\nend\n");
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n    t.integer :message_id\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", concern.as_str()),
            ("app/models/message.rb", model.as_str()),
        ];
        let error = ingest_app_from_tree(tree(&files)).expect_err("must fail strict ingest");
        assert!(error.to_string().contains("concern attr_accessor"), "{error}");
        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.expect("survey continues after refusal");
        assert!(gaps.iter().any(|gap| gap.to_string().contains("concern attr_accessor")), "{gaps:?}");
        assert!(app.models.iter().all(|model| model.body.iter().all(|item| {
            !matches!(item, ModelBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
                ExprNode::Send { method, .. } if method.as_str() == "attr_accessor"))
        })), "declined declaration must not advertise accessors");
    }
}

/// Forward/inherited/dynamic model markers are source errors before splicing,
/// independently of accessor admission. Keep the canonical visibility boundary.
#[test]
fn source_visibility_refusals_precede_concern_accessor_admission() {
    use roundhouse::ingest::survey;
    for marker in [
        "private :scratch=", "private \"scratch=\"", "protected \"scratch\"",
        "private :scratch if true", "with_options do\n    protected :scratch=\n  end",
        "private [:scratch].first",
    ] {
        let model = format!("class Message < ApplicationRecord\n  include DraftState\n  {marker}\nend\n");
        let mut files = tree(&[
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", "module DraftState\n  extend ActiveSupport::Concern\n  included { attr_accessor :scratch }\nend\n"),
            ("app/models/message.rb", &model),
        ]);
        let error = ingest_app_from_tree(files.clone()).unwrap_err().to_string();
        assert!(error.contains("app/models/message.rb") && error.contains("visibility"), "{error}");
        survey::activate();
        let result = ingest_app_from_tree(files.clone());
        let gaps = survey::drain();
        let app = result.unwrap();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].to_string(), error);
        assert!(app.models.is_empty(), "the unsupported source model is not retained");
        files.insert("app/models/message.rb".into(), model.replace("include DraftState", "").into_bytes());
        assert_eq!(ingest_app_from_tree(files).unwrap_err().to_string(), error,
            "the marker is unsupported even without a carried accessor");
    }
}

/// A broad included-block exception would erase these errors and leave the
/// emitted helper public. Only actual collector-owned candidates can defer.
#[test]
fn only_retained_module_candidates_defer_included_visibility() {
    for (opening, body) in [
        ("module DraftState", "included { private :helper }"),
        ("module DraftState", "included { Other.attr_accessor :scratch; private :helper }"),
        ("module DraftState", "included { arbitrary_dsl { attr_accessor :scratch }; private :helper }"),
        ("module DraftState", "included { attr_accessor :scratch }\n  included { private :helper }"),
        ("module DraftState", "class_methods { included { attr_accessor :scratch; private :helper } }"),
        ("module DraftState", "class << self; included { attr_accessor :scratch; private :helper }; end"),
        ("module DraftState", "if true; included { attr_accessor :scratch; private :helper }; end"),
        ("class DraftState", "included { attr_accessor :scratch; private :helper }"),
    ] {
        let concern = format!("{opening}\n  extend ActiveSupport::Concern\n  def helper; 'helper'; end\n  {body}\nend\n");
        let error = ingest_app_from_tree(tree(&[
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", &concern),
            ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\nend\n"),
        ])).unwrap_err().to_string();
        assert!(error.contains("conditional or dynamic visibility"), "{body}: {error}");
    }
}

#[test]
fn survey_refusal_is_per_model_not_per_concern() {
    use roundhouse::dialect::ModelBodyItem;
    use roundhouse::expr::ExprNode;
    use roundhouse::ingest::survey;

    survey::activate();
    let result = ingest_app_from_tree(tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :notes do |t|\n    t.string :scratch\n  end\nend\n"),
        ("app/models/concerns/draft_state.rb", "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\nend\n"),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\n  private\n  def unrelated\n    'helper'\n  end\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  include DraftState\nend\n"),
    ]));
    let gaps = survey::drain();
    let app = result.expect("survey ingests safe and colliding models");
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].to_string().contains("on Note"));
    for model in &app.models {
        let carried = model.body.iter().filter(|item| {
            matches!(item, ModelBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
                ExprNode::Send { method, .. } if method.as_str() == "attr_accessor"))
        }).count();
        assert_eq!(carried, usize::from(model.name.0.as_str() == "Message"));
    }
}

#[test]
fn survey_accessor_refusal_preserves_other_dsl_enums_modules_and_genuine_gaps() {
    use roundhouse::ingest::survey;

    let source = r#"module DraftState
  extend ActiveSupport::Concern
  included do
    has_many :comments
    enum :status, %i[draft ready]
    attr_accessor :scratch
    private :scratch
    defined?(feature.enabled?)
  end
end
module ExtraChecks
  extend ActiveSupport::Concern
  included do
    validates :body, presence: true
  end
end
"#;
    survey::activate();
    let result = ingest_app_from_tree(tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n    t.integer :status, default: 0\n  end\nend\n"),
        ("app/models/concerns/draft_state.rb", source),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState, ExtraChecks\nend\n"),
    ]));
    let gaps = survey::drain();
    let app = result.expect("survey must keep unrelated declarations");
    assert!(gaps.iter().any(|gap| gap.to_string().contains("concern attr_accessor")), "{gaps:?}");
    assert!(
        gaps.iter().any(|gap| gap.to_string().contains("`defined?` only supports bareword, ivar, and constant")),
        "{gaps:?}"
    );
    let model = &app.models[0];
    assert_eq!(model.associations().count(), 1);
    assert_eq!(model.validations().count(), 1);
    let mapping = model.enums.get(&roundhouse::Symbol::from("status")).expect("enum survived");
    assert_eq!(mapping.iter().map(|(name, value)| (name.as_str(), value)).collect::<Vec<_>>(), vec![
        ("draft", &roundhouse::expr::Literal::Int { value: 0 }),
        ("ready", &roundhouse::expr::Literal::Int { value: 1 }),
    ]);
    let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
    for name in ["comments", "draft?", "ready?"] {
        assert!(lowered.methods.iter().any(|method| method.name.as_str() == name), "{name}");
    }
    assert!(!lowered.methods.iter().any(|method| ["scratch", "scratch="].contains(&method.name.as_str())));
}

#[test]
fn unused_accessor_contexts_do_not_reject_an_app() {
    for body in ["private\n    attr_accessor :scratch", "with_options if: false do\n      attr_accessor :scratch\n    end"] {
        let concern = format!("module Dormant\n  extend ActiveSupport::Concern\n  included do\n    {body}\n  end\nend\n");
        ingest_app_from_tree(tree(&[
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/dormant.rb", concern.as_str()),
            ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
        ])).expect("no model executes the dormant included block");
    }
}

/// Accessors are supplied only by direct model splicing. Neither a plain
/// module nor a non-model class has that replacement for Concern execution.
#[test]
fn non_model_and_indirect_accessor_activations_are_errors() {
    use roundhouse::ingest::survey;
    for body in ["attr_accessor :scratch", "attr_accessor :scratch; private :helper"] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  def helper; 'helper'; end\n  included {{ {body} }}\nend\n");
        for (path, owner, source) in [
            ("app/models/support.rb", "Support", "class Support; include DraftState; end"),
            ("app/models/support.rb", "Support", "module Support; include DraftState; end"),
            ("app/controllers/support_controller.rb", "SupportController", "class SupportController < ApplicationController; include DraftState; end"),
            ("test/models/support_test.rb", "SupportTest", "class SupportTest < ActiveSupport::TestCase; include DraftState; end"),
            ("test/models/support_test.rb", "SupportTest::Support", "class SupportTest < ActiveSupport::TestCase; class Support; include DraftState; end; end"),
            ("app/models/support.rb", "Support", "module Wrapper; extend ActiveSupport::Concern; include DraftState; end\nclass Support; include Wrapper; end"),
            ("app/models/support.rb", "Indirect", "module Wrapper; extend ActiveSupport::Concern; include DraftState; end\nclass Indirect < ApplicationRecord; include Wrapper; end"),
            ("app/models/support.rb", "Support", "module Left; extend ActiveSupport::Concern; include DraftState; end\nmodule Right; extend ActiveSupport::Concern; include DraftState; end\nclass Support; include Left, Right; end"),
            ("app/models/support.rb", "Scope::Wrapper", "module Scope; module ActiveSupport; module Concern; end; end; module Wrapper; extend ActiveSupport::Concern; include ::DraftState; end; end"),
            ("app/models/support.rb", "Wrapper", "module Bindings; module ActiveSupport; module Concern; end; end; end; module Wrapper; include Bindings; extend ActiveSupport::Concern; include DraftState; end"),
            ("app/models/support.rb", "Wrapper", "module Wrapper; include DraftState; extend ActiveSupport::Concern; end"),
        ] {
            let files = tree(&[
                ("db/schema.rb", "ActiveRecord::Schema.define { create_table(:messages) { |t| t.string :body } }"),
                ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base; primary_abstract_class; end"),
                ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end"),
                ("app/models/concerns/draft_state.rb", &concern),
                ("app/models/concerns/safe_draft.rb", "module SafeDraft; extend ActiveSupport::Concern; included { attr_accessor :kept; validates :body, presence: true }; end"),
                ("app/models/message.rb", "class Message < ApplicationRecord; include SafeDraft; end"),
                (path, source),
            ]);
            let error = ingest_app_from_tree(files.clone()).unwrap_err().to_string();
            assert!(error.contains("concern attr_accessor") && error.contains(&format!("on {owner}")), "{source}: {error}");
            survey::activate();
            let result = ingest_app_from_tree(files);
            let gaps = survey::drain();
            let app = result.expect("survey retains the app but reports unsupported activation");
            assert_eq!(gaps.len(), 1, "{source}: {gaps:?}");
            assert_eq!(gaps[0].to_string(), error);
            let model = app.models.iter().find(|m| m.name.0.as_str() == "Message").unwrap();
            assert_eq!(model.validations().count(), 1);
            let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
            assert!(lowered.methods.iter().any(|m| m.name.as_str() == "kept="));
            if let Some(model) = app.models.iter().find(|m| m.name.0.as_str() == owner) {
                let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
                assert!(!lowered.methods.iter().any(|m| ["scratch", "scratch="].contains(&m.name.as_str())), "{source}");
            }
        }
    }
}

#[test]
fn shadowed_framework_does_not_supply_model_accessors() {
    let files = tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define { create_table(:messages) { |t| t.string :body } }"),
        ("app/models/concerns/virtual.rb", "module Scope; module ActiveSupport; module Concern; def included(base = nil, &block); nil; end; end; end; module Virtual; extend ActiveSupport::Concern; included { attr_accessor :scratch }; end; end"),
        ("app/models/message.rb", "class Message < ApplicationRecord; include Scope::Virtual; end"),
    ]);
    let error = ingest_app_from_tree(files.clone()).unwrap_err().to_string();
    assert!(error.contains("on Message") && error.contains("verified Concern"), "{error}");
    roundhouse::ingest::survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = roundhouse::ingest::survey::drain();
    let app = result.unwrap();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0].to_string(), error);
    let lowered = roundhouse::lower::lower_model_to_library_class(&app.models[0], &app.schema);
    assert!(!lowered.methods.iter().any(|m| ["scratch", "scratch="].contains(&m.name.as_str())));
}

#[test]
fn framework_overrides_cannot_manufacture_accessor_support() {
    for (before, after) in [
        ("", "def self.append_features(base); nil; end"),
        ("def self.included(base = nil, &block); nil; end", ""),
    ] {
        let source = format!("module Virtual\n  extend ActiveSupport::Concern\n  {before}\n  included {{ attr_accessor :scratch }}\n  {after}\nend\n");
        let native = std::process::Command::new("ruby").args(["-e", &format!("require 'active_support/concern'\n{source}\nclass NativeOwner; include Virtual; end\nabort 'unexpected native accessor' if NativeOwner.instance_methods.include?(:scratch=)")]).output().unwrap();
        assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
        let files = tree(&[
            ("db/schema.rb", "ActiveRecord::Schema.define { create_table(:messages) { |t| t.string :body } }"),
            ("app/models/concerns/virtual.rb", &source),
            ("app/models/message.rb", "class Message < ApplicationRecord; include Virtual; end"),
        ]);
        let error = ingest_app_from_tree(files.clone()).unwrap_err().to_string();
        assert!(error.contains("unconsumed included hook"), "{error}");
        roundhouse::ingest::survey::activate();
        let result = ingest_app_from_tree(files);
        let gaps = roundhouse::ingest::survey::drain();
        let app = result.unwrap();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].to_string(), error);
        let lowered = roundhouse::lower::lower_model_to_library_class(&app.models[0], &app.schema);
        assert!(!lowered.methods.iter().any(|m| ["scratch", "scratch="].contains(&m.name.as_str())));
    }
}

#[test]
fn deferred_concern_dependencies_stay_dormant_and_use_lexical_names() {
    let files = tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define { create_table(:messages) { |t| t.string :body } }"),
        ("app/models/concerns/draft_state.rb", "module DraftState; extend ActiveSupport::Concern; included { attr_accessor :scratch }; end"),
        ("app/models/concerns/wrapper.rb", "module Wrapper; extend ActiveSupport::Concern; include DraftState; end"),
        ("app/models/message.rb", "class Message < ApplicationRecord; end"),
        ("app/models/support.rb", "module Scope; module DraftState; def helper; 'local'; end; end; class Support; include DraftState; end; end"),
    ]);
    ingest_app_from_tree(files.clone()).expect("a Concern dependency is deferred, and the local namesake has no accessors");
    let mut shadowed = files.clone();
    shadowed.insert("app/models/support.rb".into(), b"module Scope; module DraftState; def helper; 'local'; end; end; class Message < ApplicationRecord; self.table_name = 'messages'; include DraftState; end; end".to_vec());
    let error = ingest_app_from_tree(shadowed.clone()).unwrap_err().to_string();
    assert!(error.contains("on Scope::Message") && error.contains("source-resolved"), "{error}");
    roundhouse::ingest::survey::activate();
    let result = ingest_app_from_tree(shadowed);
    let gaps = roundhouse::ingest::survey::drain();
    let app = result.unwrap();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0].to_string(), error);
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Scope::Message").unwrap();
    let lowered = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
    assert!(!lowered.methods.iter().any(|m| ["scratch", "scratch="].contains(&m.name.as_str())));
    let mut files = files;
    files.insert("app/models/support.rb".into(), b"module Scope; class Support; include ::DraftState; end; end".to_vec());
    let error = ingest_app_from_tree(files).unwrap_err().to_string();
    assert!(error.contains("on Scope::Support") && error.contains("concern attr_accessor"), "{error}");
}

#[test]
fn preload_helper_collisions_fail_strict_and_preserve_survey_associations() {
    use roundhouse::ingest::survey;
    for (carrier, scope) in [
        ("", "scope :ordered, -> { order(:id) }"),
        ("class_methods do\n    def ordered\n      order(:id)\n    end\n  end", ""),
    ] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :_preload_message\n  end\n  {carrier}\nend\n");
        let model = format!("class Comment < ApplicationRecord\n  include DraftState\n  belongs_to :message\n  {scope}\n  def self.loaded\n    ordered.includes(:message).to_a\n  end\nend\n");
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", concern.as_str()),
            ("app/models/message.rb", "class Message < ApplicationRecord\nend\n"),
            ("app/models/comment.rb", model.as_str()),
        ];
        let error = ingest_app_from_tree(tree(&files)).expect_err("late preload setter owns this name");
        assert!(error.to_string().contains("concern attr_accessor :_preload_message"), "{error}");
        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.expect("survey declines only the accessor");
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let comment = app.models.iter().find(|model| model.name.0.as_str() == "Comment").unwrap();
        assert_eq!(comment.associations().count(), 1);
        let src = ruby::emit_lowered_models(&app).into_iter().find(|file| file.path.to_string_lossy().ends_with("comment.rb")).unwrap().content;
        assert!(src.contains("def _preload_message(rec)"), "{src}");
        assert!(!src.contains("def _preload_message="), "{src}");
    }
}

#[test]
fn inherited_generated_surface_is_not_virtual_storage() {
    use roundhouse::ingest::survey;

    for (name, table_backed) in [
        ("inherited_comments", false),
        ("inherited_comments_cache", false),
        ("inherited_comments", true),
        ("inherited_comments_cache", true),
        ("__inherited_comments_cache", true),
    ] {
        let base_table = if table_backed { "create_table :message_bases do |t|\n    t.string :body\n  end" } else { "" };
        let schema = format!("ActiveRecord::Schema.define do\n  {base_table}\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n");
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :{name}\n  end\nend\n");
        let files = [
            ("db/schema.rb", schema.as_str()),
            ("app/models/concerns/draft_state.rb", concern.as_str()),
            ("app/models/message_base.rb", "class MessageBase < ApplicationRecord\n  self.abstract_class = true\n  has_many :inherited_comments, class_name: 'Comment', foreign_key: :message_id\nend\n"),
            ("app/models/message.rb", "class Message < MessageBase\n  include DraftState\nend\n"),
            ("app/models/comment.rb", "class Comment < ApplicationRecord\nend\n"),
        ];
        let error = ingest_app_from_tree(tree(&files)).expect_err("ancestor-generated names are occupied");
        assert!(error.to_string().contains(&format!("concern attr_accessor :{name}")), "{error}");
        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.expect("survey retains the generated ancestor association");
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let base = app.models.iter().find(|model| model.name.0.as_str() == "MessageBase").unwrap();
        assert_eq!(base.associations().count(), 1);
        let src = ruby::emit_lowered_models(&app).into_iter().find(|file| file.path.to_string_lossy().ends_with("message_base.rb")).unwrap().content;
        assert!(src.contains("def inherited_comments"), "{src}");
        assert_eq!(src.contains("def __inherited_comments_cache"), table_backed, "{src}");
    }
}

#[test]
fn temporal_memo_storage_is_not_virtual_storage() {
    use roundhouse::ingest::survey;

    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.datetime :created_at\n  end\nend\n"),
        ("app/models/concerns/draft_state.rb", "module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :__t_created_at\n  end\nend\n"),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\nend\n"),
    ];
    let error = ingest_app_from_tree(tree(&files)).expect_err("temporal memo owns this ivar");
    assert!(error.to_string().contains("concern attr_accessor :__t_created_at"), "{error}");
    survey::activate();
    let result = ingest_app_from_tree(tree(&files));
    let gaps = survey::drain();
    let app = result.expect("survey retains the temporal column");
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    let src = ruby::emit_lowered_models(&app).into_iter().find(|file| file.path.to_string_lossy().ends_with("message.rb")).unwrap().content;
    assert!(src.contains("@__t_created_at ||="), "{src}");
    assert!(!src.contains("def __t_created_at="), "{src}");
}

#[test]
fn user_derived_generated_names_are_occupied_but_user_storage_is_not() {
    use roundhouse::ingest::survey;

    for name in ["decorate_raw", "scratch"] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :{name}\n  end\nend\n");
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
            ("app/helpers/application_helper.rb", "module ApplicationHelper\n  def decorate(value)\n    value\n  end\nend\n"),
            ("app/views/messages/index.html.erb", "<%= decorate(raw('hello')) %>"),
            ("app/models/concerns/draft_state.rb", concern.as_str()),
            ("app/models/message_base.rb", "class MessageBase < ApplicationRecord\n  self.abstract_class = true\n  def decorate(value)\n    @scratch = value\n    value\n  end\nend\n"),
            ("app/models/message.rb", "class Message < MessageBase\n  include DraftState\nend\n"),
        ];
        let result = ingest_app_from_tree(tree(&files));
        if name == "decorate_raw" {
            let error = result.expect_err("generated user-derived method is occupied");
            assert!(error.to_string().contains("concern attr_accessor :decorate_raw"), "{error}");
        } else {
            result.expect("inherited source ivars remain user-owned");
        }
        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.unwrap();
        assert_eq!(gaps.len(), usize::from(name == "decorate_raw"), "{gaps:?}");
        let src = ruby::emit_lowered_models(&app).into_iter().find(|file| file.path.to_string_lossy().ends_with("message_base.rb")).unwrap().content;
        assert!(src.contains("def decorate_raw(value)"), "{src}");
    }
}

#[test]
fn controller_concern_permits_contribute_to_accessor_occupancy() {
    use roundhouse::ingest::survey;

    for name in ["from_params", "update_from_message_params", "scratch"] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :{name}\n  end\nend\n");
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/draft_state.rb", concern.as_str()),
            ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\nend\n"),
            ("app/controllers/concerns/message_parameters.rb", "module MessageParameters\n  extend ActiveSupport::Concern\n  def message_params\n    params.require(:message).permit(:body)\n  end\nend\n"),
            ("app/controllers/messages_controller.rb", "class MessagesController < ApplicationController\n  include MessageParameters\nend\n"),
        ];
        let result = ingest_app_from_tree(tree(&files));
        if name == "scratch" {
            result.expect("fresh virtual name remains supported");
        } else {
            assert!(result.is_err(), "{name} silently admitted before controller demand");
            assert!(result.unwrap_err().to_string().contains(&format!("concern attr_accessor :{name}")));
        }
        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.unwrap();
        assert_eq!(gaps.len(), usize::from(name != "scratch"), "{gaps:?}");
        let src = ruby::emit_lowered_models(&app).into_iter().find(|file| file.path.to_string_lossy().ends_with("message.rb")).unwrap().content;
        assert!(src.contains("def self.from_params("), "{src}");
        assert!(src.contains("def update_from_message_params("), "{src}");
        assert_eq!(src.contains("def scratch="), name == "scratch", "{src}");
    }
}

#[test]
fn accessor_probes_do_not_add_or_swallow_real_emit_diagnostics() {
    use roundhouse::diagnostic::Diagnostic;
    use roundhouse::emit::diagnostics::{push, scope};
    use roundhouse::span::Span;

    for declaration in ["", "attr_accessor :scratch"] {
        let concern = format!("module DraftState\n  extend ActiveSupport::Concern\n  included do\n    {declaration}\n  end\nend\n");
        let sentinel = Diagnostic::unsupported(Span::synthetic(), None, "sentinel", "outer diagnostic");
        let (app, diags) = scope(|| {
            push(sentinel.clone());
            ingest_app_from_tree(tree(&[
                ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
                ("app/models/concerns/draft_state.rb", concern.as_str()),
                ("app/models/message.rb", "class Message < ApplicationRecord\n  include DraftState\nend\n"),
                ("app/controllers/messages_controller.rb", "class MessagesController < ApplicationController\n  def message_params\n    params.require(:message).permit(:body, tags: [])\n  end\nend\n"),
            ])).unwrap()
        });
        assert_eq!(diags, vec![sentinel], "ingest must preserve only the outer diagnostic");
        let (_, diags) = scope(|| ruby::emit_lowered_models(&app));
        assert_eq!(diags.iter().filter(|diag| diag.message.contains("permitted key `tags`") && diag.message.contains("non-scalar")).count(), 1, "real emission must still report the dropped key: {diags:?}");
    }
}

/// Custom include callbacks execute outside the carried block. Survey
/// may discard the unsafe accessor contract, not unrelated declarations
/// or the same concern's contributions to an unaffected model.
#[test]
fn custom_included_hook_refusal_is_transactional_per_model() {
    use roundhouse::ingest::survey;

    for hook in [
        "def self.included(base); base.define_method(:scratch) { 'from callback' }; end",
        "class << self; def included(base); base.define_method(:scratch) { 'from callback' }; end; end",
        "def self.included(base, effect = base.define_method(:scratch) { 'from callback' }); nil; end",
    ] {
        // The hook is in a separate module: it can affect accessors
        // carried by another concern on the same includer too.
        let source = format!("module Virtual\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n    validates :body, presence: true\n  end\nend\nmodule Custom\n  {hook}\nend\n");
        let files = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :articles do |t|\n    t.string :body\n  end\nend\n"),
            ("app/models/concerns/virtual.rb", source.as_str()),
            ("app/models/message.rb", "class Message < ApplicationRecord\n  include Virtual, Custom\nend\n"),
            ("app/models/article.rb", "class Article < ApplicationRecord\n  include Virtual\nend\n"),
        ];
        let err = ingest_app_from_tree(tree(&files)).expect_err("custom hook must not advertise a fresh accessor contract");
        assert!(err.to_string().contains("unconsumed included hook"), "{err}");

        survey::activate();
        let result = ingest_app_from_tree(tree(&files));
        let gaps = survey::drain();
        let app = result.unwrap();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert!(gaps[0].to_string().contains("unconsumed included hook"), "{gaps:?}");
        for model in &app.models {
            assert_eq!(model.validations().count(), 1, "other DSL survives on both models");
            let class = roundhouse::lower::lower_model_to_library_class(model, &app.schema);
            for name in ["scratch", "scratch="] {
                assert_eq!(class.methods.iter().any(|method| method.name.as_str() == name),
                    model.name.0.as_str() == "Article", "{} {name}", model.name.0);
            }
        }
    }
}

#[test]
fn dormant_custom_included_hook_does_not_refuse_accessors_elsewhere() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n"),
        ("app/models/concerns/dormant.rb", "module Dormant\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :scratch\n  end\n  def self.included(base)\n    base.define_method(:scratch) { 'from callback' }\n  end\nend\n"),
        ("app/models/concerns/virtual.rb", "module Virtual\n  extend ActiveSupport::Concern\n  included do\n    attr_accessor :note\n  end\nend\n"),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  include Virtual\nend\n"),
    ])).unwrap();
    let class = roundhouse::lower::lower_model_to_library_class(&app.models[0], &app.schema);
    assert!(class.methods.iter().any(|method| method.name.as_str() == "note="));
    assert!(!class.methods.iter().any(|method| method.name.as_str() == "scratch="));
}
