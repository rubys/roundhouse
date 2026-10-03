//! A concern's CLASS-side methods reach the models and library classes
//! that include it (`splice_concern_class_methods_into_includers`).
//!
//! `include` never carries them. ActiveSupport::Concern only gets away
//! with it because `append_features` runs `base.extend ClassMethods`, and
//! the emitted modules have no Concern — so `Message.create_with_attachment!`
//! resolved in analyze (the registry fold copies the class side onto
//! includers) and NoMethodError'd at runtime.
//!
//! The carrier is the whole distinction. `module ClassMethods` and
//! `class_methods do` are inherited; a module's OWN singletons —
//! `module_function :x`, `class << self` — are not, and after ingest's
//! flatten all four look identical.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

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
    t.string "token", null: false
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Message::Attachment, Message::Tokenized, Message::Blocklist

  def self.paged?
    true
  end
end
"#,
        ),
        (
            "app/models/message/attachment.rb",
            r#"module Message::Attachment
  extend ActiveSupport::Concern

  MAX_WIDTH = 1200

  module ClassMethods
    def create_with_attachment!(attributes)
      create!(attributes)
    end

    def widest
      MAX_WIDTH
    end

    def paged?
      false
    end
  end
end
"#,
        ),
        (
            "app/models/message/tokenized.rb",
            r#"module Message::Tokenized
  extend ActiveSupport::Concern

  class_methods do
    def from_token(token)
      find_by(token: token)
    end
  end
end
"#,
        ),
        (
            // The lobsters `EmailBlocklistValidation` shape: the module's
            // OWN singleton, which Rails does not put on an includer.
            "app/models/message/blocklist.rb",
            r#"module Message::Blocklist
  extend ActiveSupport::Concern

  def blocked?
    Message::Blocklist.on_blocklist?(body)
  end

  def on_blocklist?(text)
    text == "spam"
  end

  module_function :on_blocklist?
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn message() -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/message.rb"))
        .map(|f| f.content.clone())
        .expect("message.rb")
}

/// The shape that sent me here: campfire's
/// `Message::Attachment::ClassMethods#create_with_attachment!`.
#[test]
fn class_methods_module_reaches_the_including_model() {
    let m = message();
    assert!(
        m.contains("def self.create_with_attachment!(attributes)"),
        "`module ClassMethods` method lands on the model:\n{m}"
    );
}

/// `class_methods do` is sugar Concern turns into that same module, so
/// both spellings have to arrive.
#[test]
fn class_methods_block_reaches_the_including_model() {
    let m = message();
    assert!(
        m.contains("def self.from_token(token)"),
        "`class_methods do` method lands on the model:\n{m}"
    );
}

/// `module_function` makes a singleton on the MODULE. Rails leaves it
/// there. Copying it invented `User.email_on_blocklist?` on three
/// lobsters models before the carrier list existed.
#[test]
fn class_shift_self_reaches_the_including_model() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"messages\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n",
        ),
        (
            "app/models/message.rb",
            "class Message < ApplicationRecord\n  include Message::Window\nend\n",
        ),
        (
            "app/models/message/window.rb",
            "module Message::Window\n  extend ActiveSupport::Concern\n  class << self\n    def from_token(token)\n      token\n    end\n  end\nend\n",
        ),
    ]))
    .expect("ingest");
    let files = ruby::emit_lowered_models(&app);
    let message = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/message.rb"))
        .map(|f| f.content.clone())
        .expect("message.rb");
    assert!(
        message.contains("def self.from_token(token)"),
        "class << self method lands on the includer:\n{message}"
    );
}

#[test]
fn module_own_singletons_do_not_reach_the_includer() {
    let m = message();
    assert!(
        !m.contains("def self.on_blocklist?"),
        "a module_function singleton stays on its module:\n{m}"
    );
}

/// Ruby's ancestor order: the class's own definition beats the module's.
#[test]
fn the_models_own_class_method_wins() {
    let m = message();
    assert_eq!(
        m.matches("def self.paged?").count(),
        1,
        "exactly one paged?, the model's own:\n{m}"
    );
    assert!(m.contains("    true\n"), "the model's body survives:\n{m}");
}

/// A lifted body's bare constant resolved against the module it was
/// written in and would resolve against the MODEL once moved — the same
/// lexical trap the controller splice hit with lobsters' TIME_INTERVALS.
#[test]
fn a_lifted_body_keeps_its_modules_constants() {
    let m = message();
    assert!(
        m.contains("Message::Attachment::MAX_WIDTH"),
        "bare MAX_WIDTH is qualified to its module:\n{m}"
    );
}

const FACTORY: &str = r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
  PREFIX = "reading:"
  def self.module_only
    "not inherited"
  end
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
    def fixed
      Reading.new(label: "fixed").freeze
    end
    def prefix(value = PREFIX)
      value
    end
  end
end
"#;
const READING: &str = "class Reading < T::Struct\n  include Factory\n  PREFIX = \"local:\"\n  const :label, String\nend\n";
const PACKET: &str = "class Packet < T::Struct\n  include Factory\n  const :size, Integer\nend\n";
const CONSUMER: &str = r#"class Consumer
  def self.label
    Reading.build(label: "probe").label
  end
  def self.size
    Packet.build(size: 7).size
  end
end
"#;

fn factory_app(path: &str, declarations: &str, consumer: &str, action: &str) -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (path, FACTORY),
        ("app/services/values.rb", declarations),
        ("app/services/consumer.rb", consumer),
        ("app/controllers/gauges_controller.rb", action),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n"),
    ])).expect("ingest factory");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn return_type(app: &roundhouse::App, class: &str, method: &str) -> Ty {
    app.library_classes.iter().find(|c| c.name.0.as_str() == class).unwrap()
        .methods.iter().find(|m| m.name.as_str() == method).unwrap()
        .body.ty.clone().unwrap()
}

#[test]
fn an_inferred_concern_factory_answers_each_receiving_class_in_both_orders() {
    // Specialize before typing, rather than replacing every nominal
    // module return with SelfInstance: fixed-other factories must stay fixed.
    for declarations in [format!("{READING}{PACKET}"), format!("{PACKET}{READING}")] {
        for path in ["app/services/factory.rb", "app/helpers/factory.rb", "lib/factory.rb"] {
            let app = factory_app(path, &declarations, CONSUMER, r#"class GaugesController < ApplicationController
  def index
    @label = Reading.build(label: "probe").label
    @size = Packet.build(size: 7).size
  end
end
"#);
            let errors: Vec<_> = diagnose(&app).into_iter()
                .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error).collect();
            assert!(errors.is_empty(), "{path}: factory errors = {errors:?}");
            assert_eq!(return_type(&app, "Consumer", "label"), Ty::Str);
            assert_eq!(return_type(&app, "Consumer", "size"), Ty::Int);
            for class in ["Reading", "Packet"] {
                let expected = Ty::Class {
                    id: roundhouse::ClassId(roundhouse::Symbol::new(class)), args: vec![],
                };
                assert_eq!(return_type(&app, class, "build"), expected);
                let lc = app.library_classes.iter().find(|c| c.name.0.as_str() == class).unwrap();
                let build = lc.methods.iter().find(|m| m.name.as_str() == "build").unwrap();
                let Some(Ty::Fn { ret, .. }) = &build.signature else { panic!("factory signature") };
                assert_eq!(**ret, expected, "signature must be concrete, not SelfInstance");
                assert_eq!(return_type(&app, class, "fixed"), Ty::Class {
                    id: roundhouse::ClassId(roundhouse::Symbol::new("Reading")), args: vec![],
                });
            }
        }
    }
}

#[test]
fn factory_typos_and_wrong_class_fields_still_fail_dispatch() {
    let app = factory_app("app/services/factory.rb", &format!("{READING}{PACKET}"), CONSUMER, r#"class GaugesController < ApplicationController
  def index
    @typo = Reading.build(label: "probe").lable
    @wrong = Packet.build(size: 7).label
    @fixed = Packet.fixed.size
    @missing = Reading.bulid(label: "probe")
  end
end
"#);
    let failed: Vec<_> = diagnose(&app).into_iter().filter_map(|d| match d.kind {
        DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
            Some(format!("{}#{}", roundhouse::ide::render_ty(&recv_ty), method.as_str()))
        }
        _ => None,
    }).collect();
    for expected in ["Reading#lable", "Packet#label", "Reading#size", "Reading#bulid"] {
        assert!(failed.iter().any(|f| f == expected), "missing {expected}: {failed:?}");
    }
}

#[test]
fn an_includers_own_factory_and_constructor_win() {
    let reading = READING.replace("end\n", "  def self.build(**fields)\n    Packet.new(size: 31).freeze\n  end\nend\n");
    let app = factory_app("app/services/factory.rb", &format!("{reading}{PACKET}"), "", "");
    assert_eq!(return_type(&app, "Reading", "build"), Ty::Class {
        id: roundhouse::ClassId(roundhouse::Symbol::new("Packet")), args: vec![],
    });
    let reading = app.library_classes.iter().find(|c| c.name.0.as_str() == "Reading").unwrap();
    assert_eq!(reading.methods.iter().filter(|m| m.name.as_str() == "build").count(), 1);

    let packet = PACKET.replace("end\n", "  def self.new(**fields)\n    Reading.new(label: \"custom\")\n  end\nend\n");
    let app = factory_app("app/services/factory.rb", &format!("{READING}{packet}"), "", "");
    assert_eq!(return_type(&app, "Packet", "build"), Ty::Class {
        id: roundhouse::ClassId(roundhouse::Symbol::new("Reading")), args: vec![],
    });
}

#[test]
fn library_factory_splicing_preserves_lexical_constants_and_carrier_boundaries() {
    let mut app = factory_app("app/services/factory.rb", &format!("{READING}{PACKET}"), CONSUMER, "");
    roundhouse::lower::class_body_new::apply_class_body_new_lowering(&mut app);
    let emitted = ruby::emit_library(&app);
    for class in ["reading", "packet"] {
        let values = emitted.iter().find(|f| f.path.to_string_lossy().ends_with(&format!("/{class}.rb"))).unwrap();
        assert!(values.content.contains("value = Factory::PREFIX"), "{}", values.content);
        assert!(!values.content.contains("def self.module_only"), "{}", values.content);
        assert_eq!(values.content.matches("def self.build").count(), 1, "{}", values.content);
    }
    let factory = emitted.iter().find(|f| f.path.to_string_lossy().ends_with("factory.rb")).unwrap();
    assert!(!factory.content.contains("def self.included"), "{}", factory.content);
}

#[test]
fn only_the_complete_class_methods_bridge_is_consumed() {
    for (source, retained) in [
        (FACTORY.to_string(), false),
        (FACTORY.replace("base.extend(ClassMethods)", "base.extend(ClassMethods)\n    puts \"registered\""), true),
        (FACTORY.replace("base.extend(ClassMethods)", "base.extend(OtherMethods)"), true),
        (FACTORY.replace("base.extend(ClassMethods)", "other.extend(ClassMethods)"), true),
        (FACTORY.replace("included(base)", "included(base, **options)"), true),
        (FACTORY.replace("module ClassMethods", "module OtherMethods"), true),
    ] {
        let classes = roundhouse::ingest::library_class::ingest_library_classes(source.as_bytes(), "factory.rb").unwrap();
        let factory = classes.iter().find(|c| c.name.0.as_str() == "Factory").unwrap();
        assert_eq!(factory.methods.iter().any(|m| m.name.as_str() == "included"), retained, "{source}");
    }
}

#[test]
fn split_bridges_require_the_actual_nested_carrier_and_keep_other_callbacks() {
    let carrier = "module Factory\n  module ClassMethods\n    def build(**fields)\n      new(**fields).freeze\n    end\n  end\nend\n";
    let block_only = carrier.replace("module ClassMethods", "class_methods do");
    let other_owner = carrier.replace("module Factory", "module OtherFactory");
    let overridden = format!("{carrier}module Factory\n  def self.included(base)\n    puts \"registered\"\n  end\nend\n");
    for (hook, declaration, retained) in [
        ("base.extend(ClassMethods)", carrier, false),
        ("base.extend(ClassMethods)\n    puts \"registered\"", carrier, true),
        ("base.extend(OtherMethods)", carrier, true),
        ("base.extend(ClassMethods)", block_only.as_str(), true),
        ("base.extend(ClassMethods)", other_owner.as_str(), true),
        ("base.extend(ClassMethods)", overridden.as_str(), true),
    ] {
        let bridge = format!("module Factory\n  def self.included(base)\n    {hook}\n  end\nend\n");
        let app = ingest_app_from_tree(tree(&[
            ("lib/factory.rb", declaration),
            ("lib/factory_bridge.rb", &bridge),
        ])).expect("ingest split carrier and bridge");
        let has_hook = app.library_classes.iter()
            .filter(|c| c.name.0.as_str() == "Factory")
            .flat_map(|c| &c.methods)
            .any(|m| m.name.as_str() == "included");
        assert_eq!(has_hook, retained, "{bridge}\n{declaration}");
    }
}
