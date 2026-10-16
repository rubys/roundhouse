//! ActiveSupport's `delegate :a, to: :b` becomes real methods
//! (`ingest::delegate`).
//!
//! Rails defines these with `class_eval` at load time, so nothing about
//! them reaches an emitted tree: the declaration lands in
//! `unknown_calls` and every call to a delegated name is a bare send no
//! class defines. Where the caller rescues, the failure is invisible —
//! campfire's `message_presentation` wraps its body in `rescue
//! Exception` and returns `""`, so a missing `fragment` drew every
//! message with an EMPTY body and no error anywhere.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(helper: &str) -> String {
    let app = ingest_app_from_tree(tree(helper)).expect("ingest");
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("filter.rb"))
        .map(|f| f.content.clone())
        .expect("filter.rb")
}

fn tree(helper: &str) -> HashMap<PathBuf, Vec<u8>> {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n".to_string(),
        ),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n".to_string()),
        ("app/helpers/filter.rb", helper.to_string()),
    ];
    files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
        .collect()
}

fn emit_model(app: &mut roundhouse::App, suffix: &str) -> String {
    roundhouse::session::analyze_and_lower(app);
    ruby::emit_lowered_models(app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("missing emitted model {suffix}"))
        .content
}

#[test]
fn private_delegate_visibility_is_preserved_for_models_and_library_classes() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  has_one :profile\n  private\n  delegate :display_name, to: :profile\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\n  create_table \"profiles\" do |t|\n    t.integer \"account_id\"\n    t.string \"display_name\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/profile.rb"),
        b"class Profile < ApplicationRecord\n  belongs_to :account\n  def display_name\n    \"profile\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("lib/private_service.rb"),
        b"class PrivateService\n  attr_reader :profile\n  private\n  delegate :display_name, to: :profile\nend\n".to_vec(),
    );

    let app = ingest_app_from_tree(files).expect("ingest");
    let account = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Account")
        .expect("Account");
    let account_delegate = account
        .methods()
        .find(|method| method.name.as_str() == "display_name")
        .expect("model delegate");
    assert_eq!(
        account_delegate.visibility,
        roundhouse::dialect::MethodVisibility::Private
    );
    let service = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "PrivateService")
        .expect("PrivateService");
    let service_delegate = service
        .methods
        .iter()
        .find(|method| method.name.as_str() == "display_name")
        .expect("library-class delegate");
    assert_eq!(
        service_delegate.visibility,
        roundhouse::dialect::MethodVisibility::Private
    );
}

#[test]
fn delegate_target_surface_respects_ruby_concern_precedence() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"profiles\" do |t|\n    t.string \"name\"\n  end\n  create_table \"pages\" do |t|\n    t.integer \"profile_id\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/concerns/private_name.rb"),
        b"module PrivateName\n  private\n  def name\n    \"private\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/concerns/public_name.rb"),
        b"module PublicName\n  def name\n    \"public\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/profile.rb"),
        b"class Profile < ApplicationRecord\n  include PrivateName\n  include PublicName\nend\n"
            .to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/page.rb"),
        b"class Page < ApplicationRecord\n  belongs_to :profile\n  delegate :name, to: :profile\nend\n".to_vec(),
    );

    let app = ingest_app_from_tree(files.clone()).expect("ingest");
    let page = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Page")
        .expect("Page");
    assert!(
        page.methods().any(|method| method.name.as_str() == "name"),
        "a later public concern method must be eligible for delegation"
    );

    files.insert(
        PathBuf::from("app/models/profile.rb"),
        b"class Profile < ApplicationRecord\n  include PublicName\n  include PrivateName\nend\n"
            .to_vec(),
    );
    let app = ingest_app_from_tree(files).expect("ingest");
    let page = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Page")
        .expect("Page");
    assert!(
        page.methods().all(|method| method.name.as_str() != "name"),
        "a later private concern method must shadow the earlier public method"
    );
    assert!(page.body.iter().any(|item| matches!(
        item,
        roundhouse::dialect::ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. } if method.as_str() == "delegate")
    )), "the unsupported delegate must remain visible");
}

#[test]
fn model_delegates_decline_collection_receivers_and_targets_that_yield() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\n  create_table \"comments\" do |t|\n    t.integer \"account_id\"\n    t.string \"email\"\n  end\n  create_table \"profiles\" do |t|\n    t.integer \"account_id\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  has_many :comments\n  has_one :profile\n  delegate :email, to: :comments, prefix: true\n  delegate :render, :block_sensitive?, to: :profile\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/comment.rb"),
        b"class Comment < ApplicationRecord\n  belongs_to :account\n  def email\n    \"comment@example.test\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/profile.rb"),
        b"class Profile < ApplicationRecord\n  belongs_to :account\n  def render\n    yield\n  end\n  def block_sensitive?\n    block_given?\n  end\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let output = emit_model(&mut app, "account.rb");
    assert!(
        !output.contains("def comments_email"),
        "a collection proxy is not a Comment instance:\n{output}"
    );
    assert!(
        !output.contains("def render"),
        "a delegate without block forwarding cannot target a yielding method:\n{output}"
    );
    assert!(
        !output.contains("def block_sensitive?"),
        "a delegate without block forwarding cannot target a method observing block_given?:\n{output}"
    );
}

/// The plain form: the forwarder CALLS the target, which is what Rails'
/// generated body does and what makes it work when the target is an
/// `attr_reader` rather than a raw ivar.
#[test]
fn a_delegated_reader_becomes_a_forwarding_method() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  def initialize(content)
    @content = content
  end

  delegate :fragment, :to_plain_text, to: :content
end
"#,
    );
    assert!(
        out.contains("def fragment\n    content.fragment\n  end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("def to_plain_text\n    content.to_plain_text\n  end"),
        "got:\n{out}"
    );
    // The declaration is CONSUMED, not replayed beside its expansion.
    assert!(!out.contains("delegate"), "got:\n{out}");
}

/// `prefix: true` is a supported ActiveSupport form. `allow_nil: true`
/// stays visible because the pass cannot model Rails' `nil.respond_to?`
/// distinction across all targets.
#[test]
fn prefix_true_is_honoured_and_allow_nil_is_declined() {
    let out = emit(
        r#"class Filter
  attr_reader :request

  delegate :host, to: :request, prefix: true
  delegate :path, to: :request, allow_nil: true
end
"#,
    );
    assert!(
        out.contains("def request_host\n    request.host\n  end"),
        "prefix should be honoured:\n{out}"
    );
    assert!(
        !out.contains("def path"),
        "allow_nil must not be lowered unsafely:\n{out}"
    );
    assert!(
        !out.contains("delegate"),
        "the DSL call must not reach emitted code:\n{out}"
    );
}

#[test]
fn explicit_symbol_and_string_prefixes_are_honoured() {
    let out = emit(
        r#"class Filter
  delegate :title, to: :article, prefix: :parent
  delegate :author, to: :article, prefix: "writer", allow_nil: false
end
"#,
    );
    assert!(
        out.contains("def parent_title\n    article.title\n  end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("def writer_author\n    article.author\n  end"),
        "got:\n{out}"
    );
}

/// DECLINED: a delegated name this class calls with ARGUMENTS. Rails
/// forwards them with `*args, &block`, which the strict targets do not
/// lower, and a zero-arg forwarder for a method that takes two is an
/// arity error standing in for a NameError. campfire's
/// `Messages::AttachmentPresentation` is exactly this shape.
#[test]
fn a_delegation_called_with_arguments_is_left_alone() {
    let out = emit(
        r#"class Filter
  attr_reader :context

  delegate :link_to, to: :context

  def render
    link_to "text", "/path"
  end
end
"#,
    );
    assert!(!out.contains("def link_to"), "got:\n{out}");
}

#[test]
fn a_delegate_to_methods_with_required_or_optional_arguments_is_left_alone() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  def formatted_name(format)\n    format\n  end\n\n  def display_name(format = :short)\n    format.to_s\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  delegate :formatted_name, :display_name, to: :account\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let emitted = emit_model(&mut app, "post.rb");
    assert!(
        !emitted.contains("def formatted_name"),
        "a zero-argument forwarder must not be synthesized for a required-argument target method:\n{emitted}"
    );
    assert!(
        !emitted.contains("def display_name"),
        "optional arguments still require forwarding and must not be dropped:\n{emitted}"
    );
}

#[test]
fn model_delegate_setters_and_fixed_arity_operators_are_synthesized() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  def +(other)\n    other\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  delegate :name=, to: :account\n  delegate :+, to: :account\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let post = emit_model(&mut app, "post.rb");
    assert!(
        post.contains("def name=(value)"),
        "setter forwarder missing:\n{post}"
    );
    assert!(
        post.contains("def +(other)"),
        "binary-operator forwarder missing:\n{post}"
    );
}

#[test]
fn a_later_unsupported_delegate_does_not_leave_an_earlier_forwarder() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/profile.rb"),
        b"class Profile < ApplicationRecord\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  belongs_to :profile\n  delegate :name, to: :account\n  delegate :name, to: :profile\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let post = emit_model(&mut app, "post.rb");
    assert!(
        !post.contains("def name\n"),
        "the earlier delegate must not override Rails' later unsupported declaration:\n{post}"
    );
}

#[test]
fn a_later_model_delegate_replaces_an_earlier_method_with_the_same_name() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  def title\n    \"account title\"\n  end\nend\n"
            .to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/page.rb"),
        b"class Page < ApplicationRecord\n  belongs_to :account\n  def title\n    \"local title\"\n  end\n  delegate :title, to: :account\nend\n".to_vec(),
    );

    let mut app = ingest_app_from_tree(files).expect("ingest");
    let page = emit_model(&mut app, "page.rb");
    assert!(
        page.contains("def title\n    self.account.title\n  end"),
        "a delegate declared after a same-named method must win in source order:\n{page}"
    );
    assert!(
        !page.contains("local title"),
        "the earlier method body must not survive the later delegate:\n{page}"
    );
}

#[test]
fn class_method_calls_do_not_suppress_instance_delegates() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  def title\n    \"account title\"\n  end\nend\n"
            .to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/concerns/title_calls.rb"),
        b"module TitleCalls\n  def self.preview\n    title(\"from concern\")\n  end\nend\n"
            .to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/page.rb"),
        b"class Page < ApplicationRecord\n  include TitleCalls\n  belongs_to :account\n  def self.preview\n    title(\"from model\")\n  end\n  delegate :title, to: :account\nend\n".to_vec(),
    );

    let mut app = ingest_app_from_tree(files).expect("ingest");
    let page = emit_model(&mut app, "page.rb");
    assert!(
        page.contains("def title\n    self.account.title\n  end"),
        "class-side calls must not require instance delegate argument forwarding:\n{page}"
    );
}

#[test]
fn a_delegate_to_a_private_target_method_is_left_alone() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  private\n  def internal_name\n    \"hidden\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  delegate :internal_name, to: :account\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let emitted = emit_model(&mut app, "post.rb");
    assert!(
        !emitted.contains("def internal_name"),
        "a delegate must not expose a private association target method:\n{emitted}"
    );
}

#[test]
fn a_private_override_blocks_an_inherited_concern_method_from_delegation() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/concerns/public_name.rb"),
        b"module PublicName\n  def internal_name\n    \"public\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  include PublicName\n  private\n  def internal_name\n    \"hidden\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  delegate :internal_name, to: :account\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let post = ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("post.rb"))
        .expect("post.rb")
        .content;
    assert!(
        !post.contains("def internal_name"),
        "a private override must shadow the concern's public method and block the delegate:\n{post}"
    );
}

#[test]
fn a_public_model_override_remains_available_for_delegation() {
    let mut files = tree("");
    files.insert(
        PathBuf::from("app/models/concerns/private_name.rb"),
        b"module PrivateName\n  private\n  def internal_name\n    \"hidden\"\n  end\nend\n"
            .to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/account.rb"),
        b"class Account < ApplicationRecord\n  include PrivateName\n  def internal_name\n    \"visible\"\n  end\nend\n".to_vec(),
    );
    files.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord\n  belongs_to :account\n  delegate :internal_name, to: :account\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let post = ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("post.rb"))
        .expect("post.rb")
        .content;
    assert!(
        post.contains("def internal_name\n    self.account.internal_name\n  end"),
        "a public model override must outrank the concern's private method:\n{post}"
    );
}

/// DECLINED: an option this pass does not reproduce. Half-expanding a
/// declaration is worse than leaving it visible.
#[test]
fn an_unreproducible_option_is_left_alone() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  delegate :fragment, to: :content, private: true
end
"#,
    );
    assert!(!out.contains("def fragment"), "got:\n{out}");
}

#[test]
fn an_invalid_explicit_prefix_is_left_alone() {
    let out = emit(
        r#"class Filter
  delegate :title, to: :article, prefix: "bad;raise"
end
"#,
    );
    assert!(
        !out.contains("def bad"),
        "invalid prefix must not create source code:\n{out}"
    );
}

/// A method the class writes itself WINS — the expansion fills gaps, it
/// does not overwrite.
#[test]
fn a_hand_written_method_is_not_replaced() {
    let out = emit(
        r#"class Filter
  attr_reader :content

  delegate :fragment, to: :content

  def fragment
    "already mine"
  end
end
"#,
    );
    assert!(out.contains("\"already mine\""), "got:\n{out}");
    assert_eq!(out.matches("def fragment").count(), 1, "got:\n{out}");
}

/// `to: :class` (or any other Ruby keyword) is `self.class` in the
/// forwarder, as Rails writes it: a bare `class.label` does not parse.
/// Shopify core has dozens (`delegate :context, to: :class`).
#[test]
fn a_keyword_target_is_read_through_self() {
    let out = emit(
        r#"class Filter
  attr_reader :return
  delegate :label, to: :class
  delegate :id, to: :return

  def self.label
    "f"
  end
end
"#,
    );
    assert!(out.contains("self.class.label"), "got:\n{out}");
    assert!(out.contains("self.return.id"), "got:\n{out}");
}

#[test]
fn a_delegated_writer_forwards_its_value() {
    // billing's `delegate :request_id, :request_id=, to: :class`: the
    // writer needs a parameter, and a zero-arg `def request_id=` is not
    // Ruby at all — the snippet failed to parse and took the reader
    // down with it.
    let out = emit(
        r#"class Filter
  delegate :request_id, :request_id=, to: :class

  def self.request_id
    @request_id
  end
end
"#,
    );
    assert!(out.contains("self.class.request_id"), "got:\n{out}");
    assert!(out.contains("self.class.request_id = value"), "got:\n{out}");
}

#[test]
fn delegated_operators_forward_their_operands() {
    // Binary operators have a fixed single-argument contract; unary
    // operators need no positional forwarding and remain valid delegates.
    let out = emit(
        r#"class Filter
  delegate :<<, :==, :!, :-@, to: :@set

  def initialize
    @set = {}
  end
end
"#,
    );
    assert!(out.contains("@set << other"), "got:\n{out}");
    assert!(out.contains("@set == other"), "got:\n{out}");
    assert!(out.contains("def !\n    !(@set)\n  end"), "got:\n{out}");
    assert!(out.contains("def -@\n    @set.-@\n  end"), "got:\n{out}");
}

#[test]
fn a_fixed_arity_operator_call_does_not_suppress_its_delegate() {
    let out = emit(
        r#"class Filter
  delegate :+, to: :@value

  def initialize
    @value = 1
  end

  def add(other)
    self + other
  end
end
"#,
    );
    assert!(
        out.contains("def +(other)\n    @value + other\n  end"),
        "the fixed-arity operator delegate must remain available to its caller:\n{out}"
    );
}

#[test]
fn variable_arity_index_operators_are_left_unexpanded() {
    let out = emit(
        r#"class Filter
  delegate :[], :[]=, to: :@items
end
"#,
    );
    assert!(
        !out.contains("def []"),
        "variable-arity indexers need forwarding:\n{out}"
    );
    assert!(
        !out.contains("def []="),
        "variable-arity indexers need forwarding:\n{out}"
    );
}

fn run_emitted(source: &str, exercise: &str) -> String {
    let emitted = emit(source);
    let script = format!("{emitted}\n{exercise}");
    let output = std::process::Command::new("ruby")
        .args(["-e", &script])
        .output()
        .expect("ruby");
    assert!(
        output.status.success(),
        "{}\n{script}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn writer_arguments_do_not_shadow_the_target_method() {
    let source = r#"class Filter
  attr_reader :value
  def initialize(value)
    @value = value
  end
  delegate :title=, to: :value
end
"#;
    assert_eq!(
        run_emitted(
            source,
            "target = Struct.new(:title).new; Filter.new(target).title = 'changed'; puts target.title"
        ),
        "changed\n"
    );
}

#[test]
fn allow_nil_true_is_not_lowered_to_an_incomplete_nil_guard() {
    let out = emit(
        r#"class Filter
  attr_reader :request

  delegate :to_s, to: :request, allow_nil: true
end
"#,
    );
    assert!(
        !out.contains("def to_s"),
        "nil implements `to_s`, so a nil guard would differ from Rails:\n{out}"
    );
}
