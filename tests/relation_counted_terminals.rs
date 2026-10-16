//! `first(n)` / `last(n)` on a relation are different methods from the
//! bare forms (`scope_chain::counted_terminal`).
//!
//! Rails' counted forms answer an Array of up to n records; the bare
//! forms answer one record or nil. One method cannot carry both return
//! types on a strict target, so the runtime splits them into `first_n` /
//! `last_n` and the call site is renamed here.
//!
//! The rename is gated on the receiver having been PROVEN a relation,
//! because `Array#first(n)` and `String#split.last(n)` mean exactly what
//! Rails means and must survive untouched — lobsters'
//! `parsed.to_html.split.first(words * 2)` is the shape a receiver-blind
//! rename would corrupt.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
    t.integer "room_id", null: false
    t.string "created_at", null: false
  end
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :messages
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  PAGE_SIZE = 20

  scope :ordered, -> { order(:created_at) }
  scope :last_page, -> { ordered.last(PAGE_SIZE) }
  scope :first_page, -> { ordered.first(PAGE_SIZE) }
  scope :newest, -> { ordered.last }
  scope :search, ->(q) { where("body like ?", q) }
  scope :shared, -> { where(room_id: 1) }

  def self.paged?
    count > PAGE_SIZE
  end

  def self.opening
    first(2)
  end
end
"#,
        ),
        (
            // A SECOND model declaring `shared` — the name now names
            // nothing, which is what pins the uniqueness guard below.
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  scope :shared, -> { where(room_id: 1) }
end
"#,
        ),
        (
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def show
    @room = Room.find(params[:id])
    @head = @room.messages.ordered.first(3)
    @paged = @room.messages.paged?
    @words = summary.split.first(4).join(" ")
    @tail = summary.split.last(2).join(" ")
    @found = anything.search("hi").last(100)
    @shared = anything.shared.last(100)
    @opening = Message.first(2)
    @closing = Message.last(3)
    @single = Message.first
  end

  private
    def summary
      "a b c d e f"
    end

    # Deliberately untypeable — the point of the two assertions that
    # read through it is that the RECEIVER's type never resolves.
    def anything
      Current.whatever
    end
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

/// The shape that sent me here: campfire's
/// `scope :last_page, -> { ordered.last(PAGE_SIZE) }`, where the receiver
/// is the threaded `__rel`.
#[test]
fn counted_terminal_on_a_threaded_relation_is_renamed() {
    let message = emitted(&ruby::emit_lowered_models(&app()), "app/models/message.rb");
    assert!(
        message.contains("Message.ordered(__rel).last_n(PAGE_SIZE)"),
        "last(n) on a relation becomes last_n:\n{message}"
    );
    assert!(
        message.contains("Message.ordered(__rel).first_n(PAGE_SIZE)"),
        "first(n) on a relation becomes first_n:\n{message}"
    );
    assert!(
        message.contains("__rel.more_than?(PAGE_SIZE)"),
        "count > PAGE_SIZE becomes more_than?:\n{message}"
    );
    assert!(
        !message.contains("__rel.count >"),
        "the COUNT comparison must not remain:\n{message}"
    );
}

/// The bare forms answer one record and keep their names — only the
/// counted forms are split.
#[test]
fn bare_terminal_keeps_its_name() {
    let message = emitted(&ruby::emit_lowered_models(&app()), "app/models/message.rb");
    assert!(
        message.contains("Message.ordered(__rel).last\n")
            || message.contains("Message.ordered(__rel).last "),
        "zero-arg last is untouched:\n{message}"
    );
    assert!(!message.contains(".last_n\n"), "no arg-less last_n:\n{message}");
}

/// A counted terminal on a has_many read rides the FK seed.
///
/// The chain names a scope (`ordered`), which is what opens
/// `apply_scope_lowering`'s gate for this body — a body whose ONLY
/// relation surface were the bare `@room.messages.first(3)` is not
/// rewritten at all today; see the note on `mentions_assoc_constructor`.
#[test]
fn counted_terminal_on_a_seeded_association_is_renamed() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains("Message.ordered(ActiveRecord::Relation.new(Message).where(room_id: @room.id).preloaded(@room.messages_target, @room.messages_loaded?)).first_n(3)"),
        "@room.messages.ordered.first(3) seeds and renames:\n{show}"
    );
}

/// campfire `Page.load(relation, :last, size)` after the selector is
/// grounded: `relation.skip_preloading!.last(size)`. The parameter is
/// untyped, but `skip_preloading!` is Relation-only — rename anyway.
#[test]
fn counted_terminal_through_skip_preloading_on_untyped_param_is_renamed() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def self.load_page(relation, direction, size)
    case direction
    when :first
      relation.skip_preloading!.first(size)
    when :last
      relation.skip_preloading!.last(size)
    end
  end
end
"#,
        ),
        (
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def index
    Widget.load_page(Widget.order(:name), :last, 2)
    render plain: "ok"
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    // Full analyze+lower — emit_lowered_models alone only rewrites
    // scope bodies via apply_scope_lowering; class-method counted
    // terminals need relation_counted_terminal on the App first.
    let mut app = app;
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    roundhouse::lower::apply_post_analyze_lowerings(&mut app, analyzer.class_registry());
    let widget = emitted(&ruby::emit_lowered_models(&app), "app/models/widget.rb");
    assert!(
        widget.contains("last_n(size)") && widget.contains("first_n(size)"),
        "skip_preloading!.last/first(size) must rename:\n{widget}"
    );
    assert!(
        !widget.contains(".last(size)") && !widget.contains(".first(size)"),
        "counted forms must not remain:\n{widget}"
    );
}

/// Without a Relation-typed seed *or* a `skip_preloading!` hop, do not
/// rename. No call site here, so the parameter stays untyped — the hop
/// is what unlocked campfire; bare `.last(size)` must not freeload.
#[test]
fn counted_terminal_on_bare_untyped_param_is_left_alone() {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def self.take_last(relation, size)
    relation.last(size)
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    let mut app = app;
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    roundhouse::lower::apply_post_analyze_lowerings(&mut app, analyzer.class_registry());
    let widget = emitted(&ruby::emit_lowered_models(&app), "app/models/widget.rb");
    assert!(
        widget.contains("relation.last(size)"),
        "bare untyped param must keep .last(size):\n{widget}"
    );
    assert!(
        !widget.contains("last_n"),
        "must not rename without skip_preloading!:\n{widget}"
    );
}

/// The gate. `String#split` answers an Array, whose `first(n)`/`last(n)`
/// already mean what Rails means — renaming them would call a method
/// Array does not have.
#[test]
fn counted_terminal_on_a_non_relation_receiver_is_left_alone() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains("split.first(4)") && show.contains("split.last(2)"),
        "Array receivers keep Array#first/#last:\n{show}"
    );
}


/// A receiver whose own type never resolves, but whose OUTERMOST call
/// NAMES A SCOPE, is a relation — a scope returns one by construction.
///
/// campfire's search page is
/// `Current.user.reachable_messages.search(query).last(100)`, where
/// `Current.user` is untyped at harvest (an ivar on a lowered
/// CurrentAttributes class) and takes the whole chain down with it. At
/// run time every link answers a real Relation; only this rename was
/// missing, so the call landed on the runtime's ZERO-ARG `last`.
#[test]
fn a_receiver_naming_a_scope_is_a_relation_even_when_untyped() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains(".search(\"hi\").last_n(100)"),
        "a scope-named receiver renames the counted terminal:\n{show}"
    );
}

/// The guard: a scope name TWO models declare names nothing, so it
/// proves nothing about the receiver. Same standard
/// `owner_model_from_name` holds association names to.
#[test]
fn a_scope_name_two_models_share_proves_nothing() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains(".shared.last(100)"),
        "an ambiguous scope name must not rename:\n{show}"
    );
    assert!(
        !show.contains(".shared.last_n(100)"),
        "an ambiguous scope name must not rename:\n{show}"
    );
}

/// The class-side counted forms: `Message.first(2)` is Rails'
/// `Message.all.first(2)`, an Array of two. The runtime's class `first`
/// takes no count, so the count rides a fresh relation's `first_n`; the
/// bare `Message.first` keeps the class method.
#[test]
fn counted_terminal_on_a_model_class_seeds_a_relation() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains("@opening = ActiveRecord::Relation.new(Message).first_n(2)"),
        "Message.first(2) seeds a relation:\n{show}"
    );
    assert!(
        show.contains("@closing = ActiveRecord::Relation.new(Message).last_n(3)"),
        "Message.last(3) seeds a relation:\n{show}"
    );
    assert!(show.contains("@single = Message.first\n"), "bare Message.first is untouched:\n{show}");
}

/// The same inside the model's own class method, where `first(2)` has
/// an implicit self. The method takes the relation it is called on (as
/// `paged?` does), defaulting to the whole table, so
/// `room.messages.opening` stays scoped.
#[test]
fn counted_terminal_with_implicit_class_self_rides_the_relation() {
    let message = emitted(&ruby::emit_lowered_models(&app()), "app/models/message.rb");
    assert!(
        message.contains("def self.opening(__rel = ActiveRecord::Relation.new(self))\n    __rel.first_n(2)\n"),
        "a bare first(2) in a class method is a relation's first_n:\n{message}"
    );
}

/// Run: `Widget.first(2)` / `.last(2)` from app code, and a class method's
/// bare `first(2)`, answer Rails' Arrays (first/last two by primary key).
/// Before the class-side rename, `Widget.first(2)` reached the runtime's
/// zero-argument `Base.first` and raised ArgumentError.
#[test]
fn counted_terminal_on_a_model_class_runs() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def self.opening\n    first(2)\n  end\nend\n")
        .write(
            "app/models/widget_report.rb",
            "class WidgetReport\n  def self.heads\n    [Widget.first(2), Widget.last(2), Widget.opening].map { |ws| ws.map { |w| w.name }.join(\",\") }.join(\"|\")\n  end\nend\n",
        )
        .run_ruby(
            "%w[alpha beta gamma].each { |n| Widget.create!(name: n) }\n\
             got = WidgetReport.heads\n\
             raise \"counted class terminals: #{got}\" unless got == \"alpha,beta|beta,gamma|alpha,beta\"\n\
             puts \"counted class terminals passed\"\n",
        )
        .assert_passes();
}
