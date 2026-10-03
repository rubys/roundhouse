//! Admission must include methods generated after source typing, not
//! merely the schema/DSL methods available at initial ingest.

use roundhouse::ingest::ingest_app_from_tree;

fn files(include: &str) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
    [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :title, null: false\n  end\nend\n".to_string(),
        ),
        (
            "app/models/concerns/virtual.rb",
            "module Virtual\n  extend ActiveSupport::Concern\n  included { attr_accessor :as_json_str }\nend\n".to_string(),
        ),
        (
            "app/models/widget.rb",
            format!("class Widget < ApplicationRecord\n  {include}\n  def as_json(options = {{}})\n    keys = [:title]\n    json = {{}}\n    keys.each {{ |key| json[key] = send(key) }}\n    json\n  end\nend\n"),
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(1)\n    render json: @widget\n  end\nend\n".to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw { resources :widgets }\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.into_bytes()))
    .collect()
}

#[test]
fn a_post_analysis_json_writer_is_not_fresh_virtual_storage() {
    let mut control = ingest_app_from_tree(files("")).expect("ordinary model ingests");
    roundhouse::session::analyze_and_lower(&mut control);
    let model = &control.models[0];
    assert!(
        model.methods().any(|method| {
            method.name.as_str() == "as_json_str" && method.name_span.is_synthetic()
        }),
        "the independent control must actually demand the generated writer"
    );

    let error = ingest_app_from_tree(files("include Virtual"))
        .expect_err("a demanded generated writer must not become an ordinary virtual getter");
    assert!(
        error
            .to_string()
            .contains("concern attr_accessor :as_json_str"),
        "{error}"
    );
}

#[test]
fn reopened_models_do_not_consume_the_same_ownership_surface_twice() {
    let mut input = files("include Virtual");
    input.insert(
        "app/models/widget_extra.rb".into(),
        b"class Widget < ApplicationRecord\n  def label\n    'reopened'\n  end\nend\n".to_vec(),
    );
    let error = ingest_app_from_tree(input)
        .expect_err("a reopened model must report its collision, not panic");
    assert!(
        error.to_string().contains("concern attr_accessor :as_json_str"),
        "{error}"
    );
}

/// A unique surface entry avoids the panic, but emission still does not
/// merge model reopenings. Fresh names must be refused rather than lost.
#[test]
fn duplicate_model_names_cannot_advertise_a_fresh_accessor() {
    use roundhouse::dialect::ModelBodyItem;
    use roundhouse::expr::ExprNode;
    use roundhouse::ingest::survey;

    let mut input = files("include Virtual");
    input.insert("app/models/concerns/virtual.rb".into(),
        b"module Virtual\n  extend ActiveSupport::Concern\n  included { attr_accessor :scratch }\nend\n".to_vec());
    let source = String::from_utf8(input[&std::path::PathBuf::from("app/models/widget.rb")].clone()).unwrap();
    input.insert("app/models/widget_extra.rb".into(), source.replace("include Virtual", "").into_bytes());
    let mut dormant = input.clone();
    dormant.insert("app/models/widget.rb".into(), source.replace("include Virtual", "").into_bytes());
    ingest_app_from_tree(dormant).expect("duplicate definitions alone are not a global admission refusal");
    let error = ingest_app_from_tree(input.clone()).err().expect("ambiguous owners cannot promise accessors");
    assert!(error.to_string().contains("single definition"), "{error}");
    survey::activate();
    let result = ingest_app_from_tree(input);
    let gaps = survey::drain();
    let app = result.unwrap();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].to_string().contains("single definition"), "{gaps:?}");
    assert!(app.models.iter().all(|model| model.body.iter().all(|item|
        !matches!(item, ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "attr_accessor"))
    )), "survey must not advertise a refused accessor");
}

#[test]
fn lexical_constant_resolution_preserves_post_analysis_ownership() {
    fn input(include: &str) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
        let mut input = files("");
        input.insert("app/services/labels.rb".into(),
            b"class Label\n  def self.value\n    [1]\n  end\nend\nmodule API\n  class Label\n    def self.value\n      'lexical'\n    end\n  end\nend\n".to_vec());
        input.insert("app/models/widget.rb".into(), format!(
            "module API\n  class Widget < ApplicationRecord\n    self.table_name = 'widgets'\n    {include}\n    def as_json(options = {{}})\n      keys = [:title]\n      json = {{}}\n      keys.each {{ |key| json[key] = send(key) }}\n      json[:label] = Label.value\n      json\n    end\n  end\nend\n"
        ).into_bytes());
        input.insert("app/controllers/widgets_controller.rb".into(),
            b"class WidgetsController < ApplicationController\n  def show\n    @widget = API::Widget.find(1)\n    render json: @widget\n  end\nend\n".to_vec());
        input
    }
    let mut control = ingest_app_from_tree(input("")).unwrap();
    roundhouse::session::analyze_and_lower(&mut control);
    let widget = control.models.iter().find(|model| model.name.0.as_str() == "API::Widget").unwrap();
    assert!(
        widget.methods().any(|method| method.name.as_str() == "as_json_str" && method.name_span.is_synthetic()),
        "the lexical String value, not the top-level Array value, must enable the writer"
    );
    let error = ingest_app_from_tree(input("include Virtual"))
        .expect_err("admission must observe the same lexical JSON writer as emission");
    assert!(error.to_string().contains("concern attr_accessor :as_json_str"), "{error}");
}

#[test]
fn accessor_typing_must_not_be_replaced_by_an_inherited_reader() {
    fn inherited(include: &str) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
        let mut input = files(include);
        input.insert("app/models/widget_base.rb".into(),
            b"class WidgetBase < ApplicationRecord\n  self.abstract_class = true\n  def scratch\n    [1]\n  end\nend\n".to_vec());
        input.insert("app/models/widget.rb".into(), format!(
            "class Widget < WidgetBase\n  attr_accessor :scratch\n  {include}\n  def as_json(options = {{}})\n    keys = [:title]\n    json = {{}}\n    keys.each {{ |key| json[key] = send(key) }}\n    json[:scratch_number] = scratch.to_i\n    json\n  end\nend\n"
        ).into_bytes());
        input
    }
    let mut control = ingest_app_from_tree(inherited("")).unwrap();
    roundhouse::session::analyze_and_lower(&mut control);
    let widget = control
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Widget")
        .unwrap();
    assert!(
        widget
            .methods()
            .any(|method| method.name.as_str() == "as_json_str"),
        "the ordinary local accessor actually enables the generated JSON writer"
    );
    let error = ingest_app_from_tree(inherited("include Virtual"))
        .expect_err("masking declarations must not change the analysis environment");
    assert!(
        error
            .to_string()
            .contains("concern attr_accessor :as_json_str"),
        "{error}"
    );
}

#[test]
fn a_late_user_override_cannot_hide_preload_ownership() {
    fn input(include: &str) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
        [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n    t.integer :message_id\n  end\nend\n".to_string()),
            ("app/models/concerns/virtual.rb", "module Virtual\n  extend ActiveSupport::Concern\n  included { attr_accessor :_preload_message }\nend\n".to_string()),
            ("app/models/message.rb", format!("class Message < ApplicationRecord\n  belongs_to :message, optional: true\n  scope :ordered, -> {{ order(:id) }}\n  {include}\n  def _preload_message(rec)\n    nil\n  end\nend\n")),
            ("app/controllers/messages_controller.rb", "class MessagesController < ApplicationController\n  def index\n    @messages = Message.ordered.includes(:message).to_a\n  end\nend\n".to_string()),
        ].into_iter().map(|(path, source)| (path.into(), source.into_bytes())).collect()
    }
    let control = ingest_app_from_tree(input("")).expect("a user override alone remains allowed");
    let model = &control.models[0];
    assert!(
        model
            .methods()
            .any(|method| method.name.as_str() == "_preload_message")
    );
    // Independently execute ordinary production materialization on an
    // override-free twin: this name belongs to real framework storage,
    // not a guessed blacklist or a second observation run.
    let mut generated = input("");
    let model_source = String::from_utf8(
        generated
            .remove(&std::path::PathBuf::from("app/models/message.rb"))
            .unwrap(),
    )
    .unwrap();
    let override_body = "  def _preload_message(rec)\n    nil\n  end\n";
    assert_eq!(model_source.matches(override_body).count(), 1);
    generated.insert(
        "app/models/message.rb".into(),
        model_source.replace(override_body, "").into_bytes(),
    );
    let mut generated = ingest_app_from_tree(generated).unwrap();
    roundhouse::session::analyze_and_lower(&mut generated);
    let files = roundhouse::emit::ruby::emit_lowered_models(&generated);
    let source = &files
        .iter()
        .find(|file| file.path == std::path::Path::new("app/models/message.rb"))
        .unwrap()
        .content;
    assert!(source.contains("def _preload_message(rec)"), "{source}");
    assert!(source.contains("@message_cache = rec"), "{source}");
    let error = ingest_app_from_tree(input("include Virtual"))
        .expect_err("a later source definition must not hide the framework's generated name");
    assert!(
        error
            .to_string()
            .contains("concern attr_accessor :_preload_message"),
        "{error}"
    );
}
