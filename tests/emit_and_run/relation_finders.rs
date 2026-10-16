use super::emit_and_run;

/// Build the same lookup through each receiver-preservation path: inline,
/// assigned to a local/ivar, or returned from a controller helper. The
/// optional association makes a successful lookup distinguish NULL from a row.
fn app(finder: &str, receiver: &str) -> emit_and_run::Overlay {
    let lookup = match receiver {
        "inline" => format!("widget = Widget.includes(:category).{finder}(id: params[:id])"),
        "local" => format!("widgets = Widget.includes(:category)\n    widget = widgets.{finder}(id: params[:id])"),
        "ivar" => format!("@widgets = Widget.includes(:category)\n    widget = @widgets.{finder}(id: params[:id])"),
        "helper" => format!("widget = widget_scope.{finder}(id: params[:id])"),
        "explicit_helper" => format!("widget = self.widget_scope.{finder}(id: params[:id])"),
        _ => panic!("unknown receiver: {receiver}"),
    };
    let helper = if matches!(receiver, "helper" | "explicit_helper") {
        "\n  def widget_scope\n    Widget.includes(:category)\n  end\n"
    } else {
        ""
    };
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "categories", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.integer "category_id"
  end
end
"#)
        .write("app/models/category.rb", "class Category < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :category, optional: true\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets/:id\", to: \"widgets#show\"\nend\n")
        .write("app/controllers/widgets_controller.rb", &format!(r#"class WidgetsController < ApplicationController
  def show
    {lookup}
    if widget
      category = widget.category
      render plain: widget.name + "/" + (category ? category.name : "none")
    else
      render plain: "missing"
    end
  end
{helper}
end
"#))
}

const ASSERTIONS: &str = r#"
category = Category.create!(name: "attached")
first = Widget.create!(name: "first", category_id: category.id)
second = Widget.create!(name: "second", category_id: nil)
[[first.id, "first/attached"], [second.id, "second/none"]].each do |id, expected|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/#{id}", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "status #{status}" unless status == 200
  raise "wrong finder result: #{body.join}" unless body.join == expected
end
"#;

#[test]
fn includes_find_by_preserves_its_relation_receiver() {
    assert_finder("find_by", "inline");
}

/// Execute present/NULL association lookups, then check the terminal's
/// missing-record contract: a nil result for find_by versus a 404 for
/// find_by! and find_sole_by.
fn assert_finder(finder: &str, receiver: &str) {
    let missing = if matches!(finder, "find_by!" | "find_sole_by") {
        "raise \"missing record must be 404\" unless status == 404"
    } else {
        "raise \"missing record result\" unless status == 200 && body.join == \"missing\""
    };
    app(finder, receiver)
        .run_ruby(&format!(r#"{ASSERTIONS}
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/999", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
{missing}
"#))
        .assert_passes();
}

#[test]
fn includes_find_by_bang_preserves_its_relation_receiver() {
    assert_finder("find_by!", "inline");
}

#[test]
fn relation_finders_preserve_local_receivers() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "local");
    }
}

#[test]
fn relation_finders_preserve_ivar_receivers() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "ivar");
    }
}

#[test]
fn relation_finders_preserve_helper_return_values() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "helper");
    }
}

#[test]
fn relation_finders_preserve_explicit_self_helper_return_values() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "explicit_helper");
    }
}

/// Shapes 1 and 2 of #558: `Part.includes(:widget).named("b").first` (an
/// app scope on a class chain) and `Part.includes(:widget).find_by_id(id)`
/// (a dynamic finder on a class chain) both hydrated the chain's receiver
/// into an Array before the trailing call ran, because the arel pass's
/// relation-receiver predicate didn't know the app's own scopes and
/// didn't treat `find_by_<attr>` as a finder. Controls alongside: a plain
/// has_many reader chained with `where`/`order`, and `.new` through a
/// helper-returned owner — both already worked and must keep working.
fn scope_and_dynamic_finder_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.string "name"
    t.integer "widget_id"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\n  scope :named, ->(n) { where(name: n) }\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets/:id/a_where", to: "widgets#a_where"
  get "/widgets/:id/a_new", to: "widgets#a_new"
  get "/parts/b_scope", to: "widgets#b_scope"
  get "/parts/:id/b_dyn", to: "widgets#b_dyn"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  # Control: plain has_many reader on a local, then where/order.
  def a_where
    widget = Widget.find(params[:id])
    parts = widget.parts.where(name: "b").order(:name)
    render plain: parts.map(&:name).join(",")
  end

  # Control: plain has_many reader through a helper, then new.
  def a_new
    part = current_widget.parts.new(name: "z")
    render plain: part.widget_id.to_s
  end

  # Shape 1: class chain, then an app scope.
  def b_scope
    part = Part.includes(:widget).named("b").first
    render plain: part.name
  end

  # Shape 2: class chain, then a dynamic finder.
  def b_dyn
    part = Part.includes(:widget).find_by_id(params[:id])
    render plain: part.name
  end

  private

  def current_widget
    Widget.find(params[:id])
  end
end
"#)
}

fn scope_and_dynamic_finder_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
widget = Widget.create!(name: "w1")
a_part = Part.create!(widget: widget, name: "a")
b_part = Part.create!(widget: widget, name: "b")

controller = WidgetsController.new
controller.params = {"id" => widget.id.to_s}
controller.process_action(:a_where)
raise "control a_where: #{controller.body}" unless controller.body == "b"

controller = WidgetsController.new
controller.params = {"id" => widget.id.to_s}
controller.process_action(:a_new)
raise "control a_new: #{controller.body}" unless controller.body == widget.id.to_s

controller = WidgetsController.new
controller.process_action(:b_scope)
raise "shape 1 (app scope on a class chain): #{controller.body}" unless controller.body == "b"

controller = WidgetsController.new
controller.params = {"id" => b_part.id.to_s}
controller.process_action(:b_dyn)
raise "shape 2 (dynamic finder on a class chain): #{controller.body}" unless controller.body == "b"

puts "relation chain scope and dynamic finder passed"
"#
}

#[test]
fn relation_chain_app_scope_and_dynamic_finder_run() {
    scope_and_dynamic_finder_app()
        .run_ruby(scope_and_dynamic_finder_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn relation_chain_app_scope_and_dynamic_finder_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        scope_and_dynamic_finder_assertions()
    );
    scope_and_dynamic_finder_app().run_spinel(&script).assert_passes();
}

/// #569 follow-up. App scope names are collected across every model and
/// matched by name, so `@widget.gadget` (a belongs_to reader) matched an
/// unrelated `Gizmo.gadget` scope and marked `@widget` relation-refined.
/// `@widget = Widget.all.find { … }` then stayed on the runtime Relation,
/// whose `find` takes an id (ArgumentError). The `show` action is the
/// older half, which the collision happened to mask: without `Gizmo`,
/// `Widget.includes(:gadget).find(id)` hydrated its receiver into an
/// Array, and `Array#find(ifnone)` answered an Enumerator.
fn scope_name_collision_app(with_gizmo: bool) -> emit_and_run::Overlay {
    let app = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "gadgets", force: :cascade do |t|
    t.string "label", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.integer "gadget_id", null: false
  end
  create_table "gizmos", force: :cascade do |t|
    t.string "kind"
  end
end
"#)
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :gadget\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets/pick", to: "widgets#pick"
  get "/widgets/:id", to: "widgets#show"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def pick
    @widget = Widget.all.find { |w| w.name == params[:name] }
    render plain: @widget.gadget.label
  end

  def show
    widget = Widget.includes(:gadget).find(params[:id])
    render plain: widget.name + "/" + widget.gadget.label
  end
end
"#);
    if !with_gizmo {
        return app;
    }
    // `mix` assigns `@thing` from two models. Only the Gizmo assignment
    // is refined by `.gadget`; the Widget one keeps Enumerable `find`.
    app.write("app/models/gizmo.rb", "class Gizmo < ApplicationRecord\n  scope :gadget, -> { where(kind: \"gadget\") }\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets/pick", to: "widgets#pick"
  get "/widgets/:id", to: "widgets#show"
  get "/gizmos/mix", to: "gizmos#mix"
end
"#)
        .write("app/controllers/gizmos_controller.rb", r#"class GizmosController < ApplicationController
  def mix
    if params[:kind] == "gizmo"
      @thing = Gizmo.all
      render plain: @thing.gadget.size.to_s
    else
      @thing = Widget.all.find { |w| w.name == params[:name] }
      render plain: @thing.gadget.label
    end
  end
end
"#)
}

#[test]
fn scope_name_on_another_model_keeps_block_find_enumerable() {
    scope_name_collision_app(true)
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
Widget.create!(name: "a", gadget: Gadget.create!(label: "g1"))
Widget.create!(name: "b", gadget: Gadget.create!(label: "g2"))

controller = WidgetsController.new
controller.params = {"name" => "b"}
controller.process_action(:pick)
raise "pick: #{controller.body}" unless controller.body == "g2"
"#)
        .assert_passes();
}

#[test]
fn scope_refines_only_the_assignment_from_its_own_model() {
    scope_name_collision_app(true)
        .run_ruby(r#"
require_relative "app/controllers/gizmos_controller"
Gizmo.create!(kind: "gadget")
Gizmo.create!(kind: "other")
Widget.create!(name: "a", gadget: Gadget.create!(label: "g1"))
Widget.create!(name: "b", gadget: Gadget.create!(label: "g2"))

controller = GizmosController.new
controller.params = {"kind" => "gizmo"}
controller.process_action(:mix)
raise "mix gizmo: #{controller.body}" unless controller.body == "1"

controller = GizmosController.new
controller.params = {"kind" => "widget", "name" => "b"}
controller.process_action(:mix)
raise "mix widget: #{controller.body}" unless controller.body == "g2"
"#)
        .assert_passes();
}

#[test]
fn includes_find_with_id_preserves_its_relation_receiver() {
    scope_name_collision_app(false)
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
Widget.create!(name: "a", gadget: Gadget.create!(label: "g1"))
widget = Widget.create!(name: "b", gadget: Gadget.create!(label: "g2"))

controller = WidgetsController.new
controller.params = {"id" => widget.id.to_s}
controller.process_action(:show)
raise "show: #{controller.body}" unless controller.body == "b/g2"
"#)
        .assert_passes();
}

/// `Relation#sole` and `find_sole_by` (Rails 7.0+): the one matching
/// record, `RecordNotFound` when nothing matches, `SoleRecordExceeded`
/// when more than one does. The controller rescues both by class, so the
/// error constants must resolve in the emitted program as well as be
/// raised by the runtime.
fn sole_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "gadgets", force: :cascade do |t|
    t.string "label", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.integer "gadget_id"
    t.datetime "created_at"
  end
end
"#)
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :gadget, optional: true\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets/sole", to: "widgets#sole"
  get "/widgets/find_sole_by", to: "widgets#find_sole"
  get "/widgets/owned_find_sole", to: "widgets#owned_find_sole"
  get "/widgets/cached_sole", to: "widgets#cached_sole"
  get "/widgets/limit_one_sole", to: "widgets#limit_one_sole"
  get "/widgets/limit_zero_sole", to: "widgets#limit_zero_sole"
  get "/widgets/owned_find_sole_by_range", to: "widgets#owned_find_sole_by_range"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def sole
    widget = Widget.where(name: params[:name]).sole
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  def find_sole
    widget = Widget.find_sole_by(name: params[:name])
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  # Through a has_many reader, which is Array-typed until a terminal
  # that Array does not answer claims it as a query.
  def owned_find_sole
    gadget = Gadget.find(params[:gadget_id])
    widget = gadget.widgets.find_sole_by(name: params[:name])
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  # A LOADED relation's `sole` must decide from its memoized records
  # AND leave them in place afterward: the row is deleted out from
  # under it between `load` and `sole` (so `sole`'s own answer already
  # proves it read the cache, since a re-query would see nothing), and
  # a second read straight after must still see the cached row rather
  # than re-querying into the now-empty table — `first_n`'s unconditional
  # `@records = nil` would otherwise cost the relation its memo even
  # though `sole` itself never had to re-query.
  def cached_sole
    rel = Widget.where(name: params[:name])
    rel.load
    Widget.where(name: params[:name]).delete_all
    widget = rel.sole
    render plain: "one:" + widget.name + "/after:" + rel.to_a.length.to_s
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  # `limit(1).sole` must probe only 1 row, not 2: two "b" rows match,
  # so an unclamped probe would see both and raise SoleRecordExceeded.
  def limit_one_sole
    widget = Widget.where(gadget_id: params[:gadget_id].to_i, name: "b").limit(1).sole
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  # `limit(0).sole` must probe 0 rows and always read as
  # RecordNotFound, never SoleRecordExceeded.
  def limit_zero_sole
    widget = Widget.where(gadget_id: params[:gadget_id].to_i, name: "b").limit(0).sole
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end

  # Association `find_sole_by` with a Range condition: the Range must
  # convert to a `>=`/`<=` pair the same way `where`/`find_by` already
  # do, not fall through to `created_at = <range object>`.
  def owned_find_sole_by_range
    gadget = Gadget.find(params[:gadget_id])
    from = Time.utc(2024, 3, 1)
    to = Time.utc(2024, 9, 1)
    widget = gadget.widgets.find_sole_by(created_at: from..to)
    render plain: "one:" + widget.name
  rescue ActiveRecord::RecordNotFound => e
    render plain: "none:" + e.message
  rescue ActiveRecord::SoleRecordExceeded => e
    render plain: "many:" + e.message
  end
end
"#)
}

fn sole_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"
owner = Gadget.create!(label: "owner")
other = Gadget.create!(label: "other")
Widget.create!(name: "a", gadget_id: owner.id)
Widget.create!(name: "b", gadget_id: owner.id)
Widget.create!(name: "b", gadget_id: owner.id)
# Another owner's widgets: the unscoped lookups see them, the owned
# ones must not.
Widget.create!(name: "c", gadget_id: other.id)
Widget.create!(name: "c", gadget_id: other.id)
Widget.create!(name: "d", gadget_id: other.id)

[:sole, :find_sole, :owned_find_sole].each do |action|
  [["a", "one:a"], ["z", "none:Couldn't find Widget"], ["b", "many:Wanted only one Widget"]].each do |name, expected|
    controller = WidgetsController.new
    controller.params = {"name" => name, "gadget_id" => owner.id.to_s}
    controller.process_action(action)
    raise "#{action}(#{name}): #{controller.body}" unless controller.body == expected
  end
end

# Scoping: "d" exists, but not under the owner.
controller = WidgetsController.new
controller.params = {"name" => "d", "gadget_id" => owner.id.to_s}
controller.process_action(:owned_find_sole)
raise "owned_find_sole(d): #{controller.body}" unless controller.body == "none:Couldn't find Widget"

puts "sole and find_sole_by passed"
"##
}

#[test]
fn relation_sole_and_find_sole_by_run() {
    sole_app().run_ruby(sole_assertions()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn relation_sole_and_find_sole_by_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        sole_assertions()
    );
    sole_app().run_spinel(&script).assert_passes();
}

#[test]
fn find_sole_by_preserves_its_relation_receiver() {
    for receiver in ["inline", "local", "ivar", "helper", "explicit_helper"] {
        assert_finder("find_sole_by", receiver);
    }
}

/// Three CodeRabbit findings on roundhouse#633, each exercised against
/// data that only a correct fix can read back right:
///
/// - `cached_sole`: the matching row is deleted between `load` and
///   `sole`, so only reading the memoized `@records` (not a fresh
///   query) survives. Run as a controller action: `.sole` alone is
///   never rewritten away from the runtime method.
/// - The `find_sole_by` cache check runs as plain post-emit Ruby
///   (not a controller action): a `recv.find_sole_by(hash)` call whose
///   receiver types as `Relation` is rewritten to `recv.where(hash)
///   .sole` at compile time (`lower::destroy_by`, Rails' own
///   definition of the method, by design and out of scope here), so a
///   controller action never reaches the runtime `Relation
///   #find_sole_by` this finding is about. Calling it directly, the
///   way an untyped or dynamically-dispatched caller would, is what
///   actually exercises the `ensure`.
/// - `limit_one_sole` / `limit_zero_sole`: `sole`'s internal probe must
///   cap at an existing `@limit` rather than always asking for 2.
/// - `owned_find_sole_by_range`: an association `find_sole_by` with a
///   Range condition must convert it the same way `where`/`find_by` do.
fn sole_cache_limit_and_range_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"

# 1. A loaded relation's `sole` must read the memoized records, not
#    re-query: the matching row is deleted out from under it between
#    `load` and `sole`.
cache_owner = Gadget.create!(label: "cache-owner")
Widget.create!(name: "solo", gadget_id: cache_owner.id)
controller = WidgetsController.new
controller.params = {"name" => "solo"}
controller.process_action(:cached_sole)
raise "cached_sole: #{controller.body}" unless controller.body == "one:solo/after:1"

# 2. `find_sole_by`, called directly rather than through a controller
#    action — a `Relation`-typed receiver's `find_sole_by` is rewritten
#    at compile time into `where(...).sole` (Rails' own definition of
#    the method, `lower::destroy_by`, out of scope here), so a
#    compiled call never reaches the runtime `Relation#find_sole_by`
#    this finding is about. Calling it directly, the way an untyped or
#    dynamically-dispatched caller would, is what exercises its
#    `ensure`: the matching rows are deleted after the load, so only
#    the restored `@records` can make `rel.to_a` still answer 2 after
#    a rescued raise.
fsb_owner = Gadget.create!(label: "fsb-owner")
Widget.create!(name: "x", gadget_id: fsb_owner.id)
Widget.create!(name: "y", gadget_id: fsb_owner.id)
rel = Widget.where(gadget_id: fsb_owner.id)
rel.load
Widget.where(gadget_id: fsb_owner.id).delete_all
begin
  rel.find_sole_by(name: "nonexistent-xyz")
rescue ActiveRecord::RecordNotFound
end
raise "find_sole_by rescue lost the cache: #{rel.to_a.length}" unless rel.to_a.length == 2

# 3 & 4. `limit(1)/(0).sole` must cap the internal probe at the
#    existing `@limit` rather than always asking for 2.
limit_owner = Gadget.create!(label: "limit-owner")
Widget.create!(name: "b", gadget_id: limit_owner.id)
Widget.create!(name: "b", gadget_id: limit_owner.id)

controller = WidgetsController.new
controller.params = {"gadget_id" => limit_owner.id.to_s}
controller.process_action(:limit_one_sole)
raise "limit_one_sole: #{controller.body}" unless controller.body == "one:b"

controller = WidgetsController.new
controller.params = {"gadget_id" => limit_owner.id.to_s}
controller.process_action(:limit_zero_sole)
raise "limit_zero_sole: #{controller.body}" unless controller.body == "none:Couldn't find Widget"

# 5. Association `find_sole_by` with a Range condition must convert it
#    the same way `where`/`find_by` already do.
range_owner = Gadget.create!(label: "range-owner")
early = Widget.create!(name: "early", gadget_id: range_owner.id)
mid = Widget.create!(name: "mid", gadget_id: range_owner.id)
late = Widget.create!(name: "late", gadget_id: range_owner.id)
Widget.where(id: early.id).update_all(created_at: Time.utc(2024, 1, 1))
Widget.where(id: mid.id).update_all(created_at: Time.utc(2024, 6, 1))
Widget.where(id: late.id).update_all(created_at: Time.utc(2025, 1, 1))

controller = WidgetsController.new
controller.params = {"gadget_id" => range_owner.id.to_s}
controller.process_action(:owned_find_sole_by_range)
raise "owned_find_sole_by_range: #{controller.body}" unless controller.body == "one:mid"

puts "sole cache/limit/range regressions passed"
"##
}

#[test]
fn sole_cache_limit_and_range_run() {
    sole_app().run_ruby(sole_cache_limit_and_range_assertions()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn sole_cache_limit_and_range_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        sole_cache_limit_and_range_assertions()
    );
    sole_app().run_spinel(&script).assert_passes();
}
