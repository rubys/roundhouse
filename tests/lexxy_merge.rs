//! What campfire's Lexxy merge (`9a258bd`, basecamp/once-campfire#224)
//! needed from the compiler, one test per piece:
//!
//! * the `lexxy` gem's editor markup for `form.rich_text_area`, block
//!   and all (Rails' own output, captured from campfire's room page);
//! * Action Text's sanitizer allow-list as the app's boot leaves it;
//! * a namespace module whose nested class reads its constants at load
//!   time, split so the constant exists before the class is required;
//! * bare helper calls in an `ActionView::TestCase` bound to the helpers
//!   Rails mixes in (the class's namesake and its `include`s);
//! * chained attachment builders and `render_action_text_attachment`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "title"
  end
  create_table "action_text_rich_texts", force: :cascade do |t|
    t.string "name", null: false
    t.text "body"
    t.string "record_type", null: false
    t.bigint "record_id", null: false
  end
end
"#;

const LOCK_WITH_LEXXY: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    lexxy (0.9.24)\n\nDEPENDENCIES\n  lexxy (~> 0.9.24)\n";

fn app(extra: &[(&str, &str)]) -> roundhouse::App {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/message.rb", "class Message < ApplicationRecord\n  has_rich_text :body\nend\n"),
    ];
    files.extend_from_slice(extra);
    let mut app = ingest_app_from_tree(tree(&files)).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn view(app: &roundhouse::App, suffix: &str) -> String {
    roundhouse::emit::ruby::emit_lowered_views(app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("no emitted view {suffix}"))
}

const FORM: &str = r#"<%= form_with model: @message do |form| %>
  <%= form.rich_text_area :body, class: "input", data: { controller: "unfurl" } do %>
    <span>inside</span>
  <% end %>
<% end %>
"#;

const CONTROLLER: &str =
    "class MessagesController < ApplicationController\n  def new\n    @message = Message.new\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  resources :messages, only: [:new, :create]\nend\n";

#[test]
fn lexxy_renders_one_editor_holding_the_block() {
    let app = app(&[
        ("Gemfile.lock", LOCK_WITH_LEXXY),
        ("app/controllers/messages_controller.rb", CONTROLLER),
        ("config/routes.rb", ROUTES),
        ("app/views/messages/new.html.erb", FORM),
    ]);
    let out = view(&app, "messages/new.rb");
    let open = out.find("<lexxy-editor").expect(&out);
    let inside = out.find("<span>inside</span>").expect(&out);
    let close = out.find("</lexxy-editor>").expect(&out);
    assert!(open < inside && inside < close, "the block sits inside the editor:\n{out}");
    assert!(!out.contains("type=\\\"hidden\\\""), "no hidden input under Lexxy:\n{out}");
    assert!(!out.contains("trix-editor"), "{out}");
    // Rails' order: the call's options (the upload URLs inside its
    // `data:`), then id, input, name.
    let data = out.find("data-controller=").expect(&out);
    let upload = out.find("data-direct-upload-url").expect(&out);
    let id = out.find(" id=\\\"message_body\\\" input=").expect(&out);
    let name = out.find("name=\\\"message[body]\\\"").expect(&out);
    assert!(data < upload && upload < id && id < name, "{out}");
    assert!(out.contains("ActionText.lexxy_editor_value("), "the record's body is the value:\n{out}");
}

#[test]
fn without_lexxy_the_editor_is_still_trix() {
    let app = app(&[
        ("app/controllers/messages_controller.rb", CONTROLLER),
        ("config/routes.rb", ROUTES),
        (
            "app/views/messages/new.html.erb",
            "<%= form_with model: @message do |form| %>\n  <%= form.rich_text_area :body %>\n<% end %>\n",
        ),
    ]);
    let out = view(&app, "messages/new.rb");
    assert!(out.contains("<trix-editor"), "{out}");
    assert!(!out.contains("lexxy-editor"), "{out}");
}

#[test]
fn the_apps_allowed_attributes_are_read_off_its_boot() {
    // campfire's shape, without the gem: the constant resolves to its
    // literal, and the current value / defaults contribute nothing.
    let app = app(&[
        (
            "app/helpers/content_filters.rb",
            "module ContentFilters\n  EDITOR_FORMATTING_ATTRIBUTES = %w[ data-language ]\nend\n",
        ),
        (
            "lib/rails_ext/action_text_allowed_tags.rb",
            "Rails.application.config.to_prepare do\n  defaults = Class.new.include(ActionText::ContentHelper).new\n  ActionText::ContentHelper.allowed_attributes =\n    (ActionText::ContentHelper.allowed_attributes || defaults.sanitizer_allowed_attributes) | ContentFilters::EDITOR_FORMATTING_ATTRIBUTES\nend\n",
        ),
    ]);
    assert_eq!(app.content_helper_allowed_attributes, vec!["data-language".to_string()]);
}

#[test]
fn the_apps_allowed_tags_are_read_off_its_boot() {
    let app = app(&[
        (
            "app/helpers/content_filters.rb",
            "module ContentFilters\n  EDITOR_FORMATTING_TAGS = %w[ s u mark table thead tbody tfoot tr th td ]\nend\n",
        ),
        (
            "lib/rails_ext/action_text_allowed_tags.rb",
            "Rails.application.config.to_prepare do\n  defaults = Class.new.include(ActionText::ContentHelper).new\n  ActionText::ContentHelper.allowed_tags =\n    (ActionText::ContentHelper.allowed_tags || defaults.sanitizer_allowed_tags) | ContentFilters::EDITOR_FORMATTING_TAGS\nend\n",
        ),
    ]);
    assert_eq!(
        app.content_helper_allowed_tags,
        ["s", "u", "mark", "table", "thead", "tbody", "tfoot", "tr", "th", "td"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_lexxy_gem_adds_its_own_attributes() {
    let app = app(&[("Gemfile.lock", LOCK_WITH_LEXXY)]);
    assert_eq!(
        app.content_helper_allowed_attributes,
        ["controls", "poster", "data-language", "style", "value", "start"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_allowed_attributes_reach_the_runtime_hook() {
    use roundhouse::project::{target_files, BuildTarget};
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let mut app = roundhouse::ingest::ingest_app(&fixture).expect("ingest real-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    app.content_helper_allowed_attributes = vec!["data-language".to_string()];
    let files = target_files(&app, &fixture, BuildTarget::Ruby).expect("ruby files");
    let rt = &files.iter().find(|(p, _)| p == "runtime/action_text.rb").expect("action_text.rb").1;
    assert!(
        rt.contains("def self.app_allowed_attributes\n      [\"data-language\"]\n    end"),
        "the generated list replaces the default []"
    );
}

#[test]
fn the_allowed_tags_reach_the_runtime_hook() {
    use roundhouse::project::{target_files, BuildTarget};
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let mut app = roundhouse::ingest::ingest_app(&fixture).expect("ingest real-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    app.content_helper_allowed_tags = vec!["s".to_string(), "table".to_string()];
    let files = target_files(&app, &fixture, BuildTarget::Ruby).expect("ruby files");
    let rt = &files.iter().find(|(p, _)| p == "runtime/action_text.rb").expect("action_text.rb").1;
    assert!(
        rt.contains("def self.app_allowed_tags\n      [\"s\", \"table\"]\n    end"),
        "the generated tag list replaces the default []:\n{rt}"
    );
    assert!(
        rt.contains("+ app_allowed_tags"),
        "allowed_tags must append the generated hook"
    );
}

#[test]
fn a_nested_class_reading_its_namespace_constant_loads_after_it() {
    let app = app(&[
        (
            "app/helpers/content_filters.rb",
            "module ContentFilters\n  EDITOR_TAGS = %w[ s u ]\n  Chain = [ SanitizeTags ]\nend\n",
        ),
        (
            "app/helpers/content_filters/sanitize_tags.rb",
            "class ContentFilters::SanitizeTags\n  ALLOWED = %w[ a b ] + ContentFilters::EDITOR_TAGS\nend\n",
        ),
    ]);
    let files = roundhouse::emit::ruby::emit_library(&app);
    let get = |suffix: &str| {
        files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with(suffix))
            .map(|f| f.content.clone())
            .unwrap_or_else(|| panic!("{suffix}"))
    };
    let parent = get("content_filters.rb");
    let constant = parent.find("EDITOR_TAGS =").expect(&parent);
    let require = parent.find("require_relative \"content_filters/sanitize_tags\"").expect(&parent);
    let chain = parent.find("Chain =").expect(&parent);
    assert!(constant < require && require < chain, "split around the require:\n{parent}");
    // The child names its namespace file by a real path, not "".
    let child = get("content_filters/sanitize_tags.rb");
    assert!(child.contains("require_relative \"../content_filters\""), "{child}");
    assert!(!child.contains("require_relative \"\""), "{child}");
}

#[test]
fn a_chained_attachment_builder_and_the_content_helper_render_the_attachment() {
    // campfire's rewritten opengraph test: `attachments_for` answers an
    // Array of `…_attachment_for` calls, each ending in a helper whose
    // tail is `from_node`; the render sits in a `.map` block. And
    // `editable_body`'s `render_action_text_attachment(attachment)`.
    let test = r#"require "test_helper"

class EmbedTest < ActiveSupport::TestCase
  test "renders" do
    htmls = attachments_for("x").map do |attachment|
      ApplicationController.render partial: attachment.to_partial_path, locals: { user: attachment }
    end
    assert htmls
  end

  private
    def attachments_for(html)
      [ one_for(html), one_for(html) ]
    end

    def one_for(html)
      attachment_from html
    end

    def attachment_from(html)
      ActionText::Attachment.from_node ActionText::Fragment.wrap(html).find_all(ActionText::Attachment.tag_name).first
    end
end
"#;
    let app = app(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"messages\", force: :cascade do |t|\n    t.string \"title\"\n  end\n  create_table \"action_text_rich_texts\", force: :cascade do |t|\n    t.string \"name\", null: false\n    t.text \"body\"\n    t.string \"record_type\", null: false\n    t.bigint \"record_id\", null: false\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include ActionText::Attachable\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        ("app/views/users/_mention.html.erb", "<span><%= user.name %></span>\n"),
        (
            "app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  def show\n    @user = User.first\n  end\nend\n",
        ),
        ("app/views/users/show.html.erb", "<%= render partial: \"users/mention\", locals: { user: @user } %>\n"),
        (
            "app/helpers/rich_text_helper.rb",
            "module RichTextHelper\n  def rebuilt(attachment)\n    render_action_text_attachment(attachment)\n  end\nend\n",
        ),
        ("test/models/embed_test.rb", test),
    ]);
    let tm = app.test_modules.iter().find(|t| t.name.0.as_str() == "EmbedTest").expect("test module");
    let body = roundhouse::emit::ruby::emit_expr(&tm.tests[0].body);
    assert!(body.contains("ActionText::Content.render_attachment(attachment)"), "{body}");
    assert!(!body.contains("ApplicationController.render"), "{body}");
    let helper = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("rich_text_helper.rb"))
        .map(|f| f.content)
        .expect("helper");
    assert!(helper.contains("ActionText::Content.render_attachment(attachment)"), "{helper}");
}

#[test]
fn a_rich_text_record_answers_its_stored_html() {
    let app = app(&[]);
    let rich_text = roundhouse::emit::ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("action_text/rich_text.rb"))
        .map(|f| f.content)
        .expect("rich_text.rb");
    assert!(rich_text.contains("def body_before_type_cast\n    @body.to_s\n  end"), "{rich_text}");
}

#[test]
fn render_layout_false_renders_the_actions_own_template_and_a_bare_json_arm_is_kept() {
    // campfire's autocompletion endpoint since the Lexxy merge: the HTML
    // arm answers the mention prompt without the layout, the bare JSON
    // arm answers the autocomplete inputs with the action's jbuilder.
    let app = app(&[
        (
            "app/controllers/messages_controller.rb",
            "class MessagesController < ApplicationController\n  def index\n    @messages = Message.all\n    respond_to do |format|\n      format.html { render layout: false }\n      format.json\n    end\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :messages, only: [:index]\nend\n"),
        ("app/views/messages/index.html.erb", "<% @messages.each do |m| %><p><%= m.title %></p><% end %>\n"),
        ("app/views/messages/index.json.jbuilder", "json.array! @messages, :title\n"),
    ]);
    let out = roundhouse::emit::ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("messages_controller.rb"))
        .map(|f| f.content)
        .expect("controller");
    assert!(!out.contains("render(layout: false)"), "{out}");
    assert!(out.contains("render(Views::Messages.index("), "the html arm renders the template:\n{out}");
    assert!(out.contains("Views::Messages.index_json("), "the bare json arm renders the jbuilder:\n{out}");
}

#[test]
fn gem_stylesheets_are_linked_in_propshafts_filename_order() {
    let lock = "GEM\n  remote: https://rubygems.org/\n  specs:\n    action_text-trix (2.1.19)\n    lexxy (0.9.24)\n\nDEPENDENCIES\n  lexxy\n";
    let layout = |arg: &str| format!("<html><head><%= stylesheet_link_tag {arg} %></head><body><%= yield %></body></html>\n");
    let all = layout(":all");
    let app_all = app(&[
        ("Gemfile.lock", lock),
        ("app/assets/stylesheets/application.css", ""),
        ("app/views/layouts/application.html.erb", &all),
    ]);
    assert_eq!(
        app_all.stylesheets,
        ["application", "lexxy-content", "lexxy-editor", "lexxy-variables", "lexxy", "trix"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
    // `:app` is the app's own stylesheets only (Propshaft's
    // `app_stylesheets_paths`): the gems are in the bundle, not linked.
    let app_only = layout(":app");
    let app_app = app(&[
        ("Gemfile.lock", lock),
        ("app/assets/stylesheets/application.css", ""),
        ("app/views/layouts/application.html.erb", &app_only),
    ]);
    assert_eq!(app_app.stylesheets, vec!["application".to_string()]);
}
