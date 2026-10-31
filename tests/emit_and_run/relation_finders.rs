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

/// A model helper passes the nullable dynamic-finder return to a
/// controller ivar. The nil guard must leave the model, and the
/// emitted lookup must still find a row or return nil as Rails does.
#[test]
fn class_dynamic_finder_keeps_the_record_through_its_helper_and_guard() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "categories", force: :cascade do |t|
    t.string "name", null: false
    t.string "type"
  end
end
"#)
        .write("app/models/category.rb", r#"class Category < ApplicationRecord
  def self.lookup(id)
    Category.find_by_id(id)
  end
  def self.bare_lookup(id)
    find_by_id(id)
  end
  def self.explicit_lookup(id)
    self.find_by_id(id)
  end
end
"#)
        .write("app/models/special_category.rb", "class SpecialCategory < Category\n  self.table_name = \"categories\"\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/categories/:id\", to: \"categories#show\"\nend\n")
        .write("app/controllers/categories_controller.rb", r#"class CategoriesController < ApplicationController
  def show
    @category = Category.lookup(params[:id])
    if @category.nil?
      render plain: "missing"
    else
      render plain: @category.name
    end
  end
end
"#)
        .run_ruby(r##"
row = Category.create!(name: "found")
[[row.id, "found"], [999, "missing"]].each do |id, expected|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/categories/#{id}", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "status #{status}" unless status == 200
  raise "#{id}: #{body.join}" unless body.join == expected
end
raise "finder miss" unless Category.lookup(999).nil?
raise "bare finder" unless Category.bare_lookup(row.id).name == "found"
raise "self finder" unless Category.explicit_lookup(row.id).name == "found"
special = SpecialCategory.create!(name: "child")
raise "bare subclass receiver" unless SpecialCategory.bare_lookup(special.id).class == SpecialCategory
raise "explicit subclass receiver" unless SpecialCategory.explicit_lookup(special.id).class == SpecialCategory
"##)
        .assert_passes();
}

#[test]
fn class_dynamic_finder_lowering_preserves_application_overrides() {
    emit_and_run::real_blog()
        .edit("app/models/application_record.rb", "  primary_abstract_class\n", r#"  primary_abstract_class
  def self.find_by_title(title)
    "inherited:#{title}"
  end
"#)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", r#"  validates :title, presence: true
  def self.find_by_id(id)
    "own:#{id}"
  end
  def self.own_lookup(id)
    find_by_id(id)
  end
"#)
        .write("app/models/finder_probe.rb", r#"class FinderProbe
  def self.value
    Article.find_by_id(7)
  end
  def self.inherited_value
    Article.find_by_title("probe")
  end
end
"#)
        .run_ruby(r##"
raise "override rewritten" unless FinderProbe.value == "own:7"
raise "bare override rewritten" unless Article.own_lookup(8) == "own:8"
raise "inherited override rewritten" unless FinderProbe.inherited_value == "inherited:probe"
"##)
        .assert_passes();
}

/// The same sources and assertions run against Rails 8.1.4 and emitted
/// Ruby. In particular, a hit outside the scope must stay excluded.
fn review_dynamic_finder_parity(sources: &[(&str, &str)], assertions: &str) {
    review_dynamic_finder_parity_schema(sources, assertions, false);
}

#[test]
fn w088_review_unrelated_hook_dynamic_finders_are_explicitly_refused() {
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:categories) { |t| t.string :name; t.boolean :active, default: true }\nend\n";
    let hook = "module ExternalFinder\ndef self.included(klass)\nklass.define_singleton_method(:audit_label) { \"audit\" }\nend\nend\n";
    for finder in ["find_by_name", "find_by_name!"] {
        for shape in ["direct", "direct_symbol", "direct_string", "copy", "block", "self", "bare", "symbol", "string", "scoped"] {
            let local = match shape {
                "self" => format!("self.{finder}(value)"),
                "symbol" => format!("public_send(:{finder}, value)"),
                "string" => format!("__send__({finder:?}, value)"),
                _ => format!("{finder}(value)"),
            };
            let helper = if matches!(shape, "direct" | "direct_symbol" | "direct_string" | "copy" | "block") { String::new() } else { format!("def self.lookup(value); {local}; end\n") };
            let category = format!("class Category < ActiveRecord::Base\ninclude ExternalFinder\nscope :active_only, -> {{ where(active: true) }}\n{helper}end\n");
            let call = match shape {
                "direct" => format!("Category.{finder}(\"found\")"),
                "direct_symbol" => format!("Category.public_send(:{finder}, \"found\")"),
                "direct_string" => format!("Category.send({finder:?}, \"found\")"),
                "copy" => format!("klass = Category; klass.{finder}(\"found\")"),
                "block" => format!("Category.{finder}(\"found\") {{ \"unused\" }}"),
                "scoped" => "Category.active_only.lookup(\"hidden\")".to_owned(),
                _ => "Category.lookup(\"found\")".to_owned(),
            };
            let probe = format!("class FinderProbe\ndef self.value; {call}; end\nend\n");
            let check = if shape == "scoped" {
                if finder.ends_with('!') { "begin; FinderProbe.value; raise \"bang scope leaked\"; rescue ActiveRecord::RecordNotFound; end" }
                else { "raise \"scope leaked\" unless FinderProbe.value.nil?" }
            } else { "raise \"native finder\" unless FinderProbe.value.name == \"found\"" };
            let native = format!("gem \"activerecord\", \"8.1.4\"\nrequire \"active_record\"\nActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\nActiveRecord::Schema.verbose = false\n{schema}{hook}{category}{probe}Category.create!(name: \"found\", active: true)\nCategory.create!(name: \"hidden\", active: false)\n{check}\n");
            let oracle = std::process::Command::new("ruby").args(["-e", &native]).output().unwrap();
            assert!(oracle.status.success(), "{finder} {shape}, Rails 8.1.4: {}", String::from_utf8_lossy(&oracle.stderr));
            let (_emitted, errors) = emit_and_run::empty_app()
                .write("db/schema.rb", schema)
                .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
                .write("app/models/external_finder.rb", hook)
                .write("app/models/category.rb", &category)
                .write("app/models/finder_probe.rb", &probe)
                .emit(roundhouse::project::BuildTarget::Ruby);
            assert!(errors.iter().any(|error| error.contains("dynamic finder without established implementation")), "{finder} {shape}: {errors:?}");
        }
    }
    review_dynamic_finder_parity(&[
        ("app/models/external_finder.rb", hook),
        ("app/models/category.rb", "class Category < ActiveRecord::Base\ninclude ExternalFinder\nscope :active_only, -> { where(active: true) }\ndef self.lookup(value); find_by(name: value); end\nend\n"),
        ("app/models/finder_probe.rb", "class FinderProbe\ndef self.ordinary_hidden; Category.active_only.lookup(\"hidden\"); end\nend\n"),
    ], "Category.create!(name: \"hidden\", active: false)\nraise \"ordinary scope\" unless FinderProbe.ordinary_hidden.nil?\n");
}

#[test]
fn w088_review_extension_order_matches_ruby_and_rails() {
    for (extensions, first) in [
        ("extend FirstFinder, LastFinder", true),
        ("extend FirstFinder\nextend LastFinder", false),
        ("extend LastFinder, FirstFinder", false),
        ("extend LastFinder\nextend FirstFinder", true),
    ] {
        let category = format!("class Category < ActiveRecord::Base\n{extensions}\ndef self.lookup; find_by_name(\"found\"); end\nend\n");
        let probe = "class FinderProbe\ndef self.direct; Category.find_by_name(\"found\"); end\ndef self.ordinary; Category.find_by(name: \"found\"); end\ndef self.reflected; Category.send(:find_by_name, \"found\"); end\ndef self.reflected_string; Category.public_send(\"find_by_name\", \"found\"); end\nend\n";
        let owner = if first { "FirstFinder" } else { "LastFinder" };
        let mut checks = format!("raise \"singleton order\" unless Category.method(:find_by).owner == {owner}\n");
        for call in ["FinderProbe.direct", "FinderProbe.ordinary", "FinderProbe.reflected", "FinderProbe.reflected_string", "Category.lookup"] {
            checks.push_str(&if first { format!("raise \"first owner\" unless {call}.is_a?(Category) && {call}.id == 91 && {call}.name == \"first owner\"\n") } else { format!("raise \"last owner\" unless {call}.nil?\n") });
        }
        review_dynamic_finder_parity(&[
            ("app/models/first_finder.rb", "module FirstFinder\ndef find_by(attrs); Category.new(id: 91, name: \"first owner\"); end\nend\n"),
            ("app/models/last_finder.rb", "module LastFinder\ndef find_by(attrs); nil; end\nend\n"),
            ("app/models/category.rb", &category),
            ("app/models/finder_probe.rb", probe),
        ], &checks);
    }
}

#[test]
fn w088_review_retained_include_hooks_preserve_dynamic_dispatch() {
    for (hook, finder, reflection) in [
        ("find_by_name", "find_by_name", false),
        ("find_by_name!", "find_by_name!", false),
        ("find_by", "find_by", false),
        ("find_by!", "find_by!", false),
        ("method_missing", "find_by_name", false),
        ("public_send", "find_by_name", true),
    ] {
        for inclusion in ["direct", "parent", "nested"] {
            let external = format!("module ExternalFinder\ndef self.included(klass)\nklass.define_singleton_method(:{hook}) {{ |*args| \"hook owner\" }}\nend\nend\n");
            // A nested module's hook must be retained too, without relying
            // on Ruby copying its singleton methods through plain include.
            let carrier = if inclusion == "nested" { "module FinderCarrier\ninclude ExternalFinder\ndef self.included(klass)\nklass.include ExternalFinder\nend\nend\n" } else { "module FinderCarrier\nend\n" };
            let parent = format!("class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\n{}\nend\n", if inclusion == "parent" { "include ExternalFinder" } else { "" });
            let argument = if matches!(finder, "find_by" | "find_by!") { "name: \"found\"" } else { "\"found\"" };
            let call = if reflection { format!("public_send(:{finder}, {argument})") } else { format!("{finder}({argument})") };
            let category = format!("class Category < ApplicationRecord\n{}\ndef self.lookup; {call}; end\nend\n", match inclusion { "direct" => "include ExternalFinder", "nested" => "include FinderCarrier", _ => "" });
            let checks = if reflection {
                format!("raise \"hook reflection\" unless Category.public_send(:{finder}, {argument}) == \"hook owner\"\nraise \"bare hook wrapper\" unless Category.lookup == \"hook owner\"\n")
            } else {
                format!("raise \"direct hook\" unless Category.{finder}({argument}) == \"hook owner\"\nraise \"bare hook\" unless Category.lookup == \"hook owner\"\n[:send, :public_send, :__send__].each {{ |selector| raise \"reflected hook\" unless Category.send(selector, :{finder}, {argument}) == \"hook owner\" }}\n")
            };
            let checks = format!("Category.create!(name: \"found\")\n{checks}");
            review_dynamic_finder_parity(&[("app/models/external_finder.rb", &external), ("app/models/finder_carrier.rb", &carrier), ("app/models/application_record.rb", &parent), ("app/models/category.rb", &category)], &checks);
        }
    }
}

#[test]
fn w088_review_plain_include_does_not_install_singleton_delegates() {
    for extension in [false, true] {
        let definitions = if extension {
            "def find_by(attrs); \"extended\"; end\ndef find_by!(attrs); \"extended\"; end"
        } else {
            "def self.find_by(attrs); \"decoy\"; end\ndef self.find_by!(attrs); \"decoy\"; end"
        };
        let external = format!("module ExternalFinder\n{definitions}\nend\n");
        let category = format!("class Category < ActiveRecord::Base\n{} ExternalFinder\nend\n", if extension { "extend" } else { "include" });
        let mut probe = String::from("class FinderProbe\n");
        let mut checks = String::from("Category.create!(name: \"found\")\n");
        for (index, finder) in ["find_by_name", "find_by_name!"].iter().enumerate() {
            for (label, value) in [("hit", "found"), ("miss", "absent")] {
                let mut calls = vec![format!("Category.{finder}({value:?})")];
                for selector in ["send", "public_send", "__send__"] {
                    calls.push(format!("Category.{selector}(:{finder}, {value:?})"));
                    calls.push(format!("Category.{selector}({finder:?}, {value:?})"));
                }
                for (call_index, call) in calls.iter().enumerate() {
                    let name = format!("lookup_{index}_{label}_{call_index}");
                    probe.push_str(&format!("def self.{name}; {call}; end\n"));
                    if extension {
                        checks.push_str(&format!("raise \"extension\" unless FinderProbe.{name} == \"extended\"\n"));
                    } else if index == 1 && label == "miss" {
                        checks.push_str(&format!("begin; FinderProbe.{name}; raise \"bang must raise\"; rescue ActiveRecord::RecordNotFound; end\n"));
                    } else if label == "miss" {
                        checks.push_str(&format!("raise \"miss\" unless FinderProbe.{name}.nil?\n"));
                    } else {
                        checks.push_str(&format!("raise \"hit\" unless FinderProbe.{name}.name == \"found\"\n"));
                    }
                }
            }
        }
        probe.push_str("end\n");
        review_dynamic_finder_parity(&[("app/models/external_finder.rb", &external), ("app/models/category.rb", &category), ("app/models/finder_probe.rb", &probe)], &checks);
    }
}

#[test]
fn w088_review_unknown_relation_hook_dispatch_is_explicitly_refused() {
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:categories) { |t| t.string :name }\nend\n";
    let hook = "module ExternalFinder\ndef self.included(klass)\nklass.define_singleton_method(:find_by_name) { |name| \"hook owner\" }\nend\nend\n";
    let category = "class Category < ActiveRecord::Base\ninclude ExternalFinder\nend\n";
    let probe = "class FinderProbe\ndef self.lookup; Category.all.find_by_name(\"found\"); end\nend\n";
    let native = format!("gem \"activerecord\", \"8.1.4\"\nrequire \"active_record\"\nActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\nActiveRecord::Schema.verbose = false\n{schema}{hook}{category}{probe}raise \"native hook\" unless FinderProbe.lookup == \"hook owner\"\n");
    let oracle = std::process::Command::new("ruby").args(["-e", &native]).output().unwrap();
    assert!(oracle.status.success(), "Rails 8.1.4: {}", String::from_utf8_lossy(&oracle.stderr));
    let (_emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", schema)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/external_finder.rb", hook)
        .write("app/models/category.rb", category)
        .write("app/models/finder_probe.rb", probe)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|error| error.contains("unknown class-side hook")), "{errors:?}");
}

#[test]
fn w088_review_subclass_forms_keep_routes_parameter_keys_and_errors() {
    // The oracle below pins this release; unit partitions do not install
    // its optional ActionView and ActionPack components.
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let probe = std::process::Command::new("ruby")
        .env("RH_RAILS_ORACLE", "8.1.4")
        .args(["-e", &format!("require {oracle:?}\nputs RailsOracle.available?(\"activerecord\", \"actionview\", \"actionpack\", sqlite3: \">= 2.9\")")])
        .output()
        .expect("Rails oracle availability probe");
    let available = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    assert!(probe.status.success() && matches!(available.as_str(), "true" | "false"),
        "Rails oracle availability probe failed: {available}\n{}", String::from_utf8_lossy(&probe.stderr));
    if available == "false" {
        eprintln!("skipping: Rails 8.1.4 form oracle requires activerecord, actionview, actionpack and sqlite3 >= 2.9");
        return;
    }
    const SCHEMA: &str = "ActiveRecord::Schema.define do\ncreate_table(:rooms) { |t| t.string :name; t.string :type }\nend\n";
    const ROOM: &str = "class Room < ActiveRecord::Base\nattr_accessor :user_ids_input\nend\n";
    const DIRECT: &str = "class Rooms::Direct < Room\nend\n";
    const TEMPLATE: &str = "<%= form_with model: @room do |form| %><%= form.text_field :user_ids_input %><% end %>";
    const ROUTES: &str = "Rails.application.routes.draw do\nresources :rooms\nnamespace :rooms do\nresources :directs\nend\nend\n";
    const CHECK: &str = r##"
def check_form(html, condition)
  action = condition == "persisted" ? "/rooms/directs/1" : "/rooms/directs"
  raise "action #{html}" unless html.include?(%Q(action="#{action}"))
  raise "parameter key #{html}" unless html.include?('name="rooms_direct[user_ids_input]"')
  raise "input id #{html}" unless html.include?('id="rooms_direct_user_ids_input"')
  raise "error wrapper #{html}" unless html.include?('field_with_errors') == (condition == "invalid")
end
"##;
    let native = format!(r#"gem "activerecord", "8.1.4"
gem "actionview", "8.1.4"
gem "actionpack", "8.1.4"
require "active_record"
require "action_view"
require "action_controller"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{SCHEMA}
{ROOM}
module Rooms; end
{DIRECT}
routes = ActionDispatch::Routing::RouteSet.new
routes.draw do
  resources :rooms
  namespace :rooms do
    resources :directs
  end
end
ActionView::Helpers::FormHelper.form_with_generates_ids = true
view = ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new([]), {{}}, nil)
view.extend(routes.url_helpers)
view.extend(ActionDispatch::Routing::UrlFor)
view.extend(ActionDispatch::Routing::PolymorphicRoutes)
view.define_singleton_method(:_routes) {{ routes }}
view.define_singleton_method(:default_url_options) {{ {{}} }}
view.define_singleton_method(:url_options) {{ {{host: "example.test"}} }}
view.define_singleton_method(:protect_against_forgery?) {{ false }}
{CHECK}
room = Rooms::Direct.new
["new", "invalid", "persisted"].each do |condition|
  room.errors.add(:user_ids_input, "invalid users") if condition == "invalid"
  if condition == "persisted"
    room.errors.clear
    room.save!
  end
  view.assign(room: room)
  check_form(view.render(inline: {template:?}), condition)
end
"#, template = TEMPLATE);
    let oracle = std::process::Command::new("ruby").args(["-e", &native]).output().unwrap();
    assert!(oracle.status.success(), "Rails 8.1.4: {}", String::from_utf8_lossy(&oracle.stderr));
    let (emitted, app, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/room.rb", ROOM)
        .write("app/models/rooms/direct.rb", DIRECT)
        .write("config/routes.rb", ROUTES)
        .write("app/controllers/rooms/directs_controller.rb", "class Rooms::DirectsController < ActionController::Base\ndef new\n@room = Rooms::Direct.new\nend\nend\n")
        .write("app/views/rooms/directs/new.html.erb", TEMPLATE)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(app.view_ivar_types.values().any(|vars| vars.values().any(|ty|
        matches!(ty, roundhouse::ty::Ty::Class { id, .. } if id.0.as_str() == "Rooms::Direct"))), "{:?}", app.view_ivar_types);
    let script = format!(r#"require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
{CHECK}
room = Rooms::Direct.new
["new", "invalid", "persisted"].each do |condition|
  room.error_details << ActiveRecord::ErrorDetail.new("user_ids_input", "invalid users", "invalid users", "User ids input invalid users", false, true) if condition == "invalid"
  if condition == "persisted"
    room.error_details.clear
    room.id = 1
    room.mark_persisted!
  end
  check_form(Views::Rooms::Directs._new(room), condition)
end
"#);
    let output = std::process::Command::new("ruby").args(["-e", &script]).current_dir(&emitted).env("BLOG_DB", ":memory:").output().unwrap();
    assert!(output.status.success(), "emitted form: {}", String::from_utf8_lossy(&output.stderr));
}

fn review_dynamic_finder_parity_schema(sources: &[(&str, &str)], assertions: &str, with_type: bool) {
    const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table :categories do |t|
    t.string :name
    t.boolean :active, default: true
    t.integer :parent_id
  end
end
"#;
    let schema = if with_type { SCHEMA.replace("t.string :name", "t.string :name\n    t.string :type") } else { SCHEMA.to_owned() };
    let mut app = emit_and_run::empty_app()
        .write("db/schema.rb", &schema)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n");
    let mut native = String::from(r#"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
"#);
    native.push_str(&schema);
    for (path, source) in sources {
        app = app.write(path, source);
        native.push_str(source);
        native.push('\n');
    }
    native.push_str(assertions);
    let oracle = std::process::Command::new("ruby").args(["-e", &native]).output().expect("CRuby oracle");
    assert!(oracle.status.success(), "Rails 8.1.4: {}", String::from_utf8_lossy(&oracle.stderr));
    app.run_ruby(assertions).assert_passes();
}

#[test]
fn class_dynamic_finder_review_preserves_ancestor_and_missing_method_ownership() {
    for (parent, concern, declaration) in [
        ("def self.find_by_name(value); \"owned\"; end", "", ""),
        ("", "", "def self.find_by_name(value); \"owned\"; end"),
        ("def self.method_missing(name, *args); \"owned\"; end", "", ""),
        ("", "", "def self.method_missing(name, *args); \"owned\"; end"),
        ("", "def find_by_name(value); \"owned\"; end", "include FinderHooks"),
        ("", "def method_missing(name, *args); \"owned\"; end", "include FinderHooks"),
    ] {
        let parent = format!("class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\n{parent}\nend\n");
        let concern = format!("module FinderHooks\nextend ActiveSupport::Concern\nclass_methods do\n{concern}\nend\nend\n");
        let category = format!(r#"class Category < ApplicationRecord
  {declaration}
  def self.bare; find_by_name("found"); end
  def self.explicit; self.find_by_name("found"); end
  def self.reflected; send(:find_by_name, "found"); end
  def self.public_reflected; self.public_send(:find_by_name, "found"); end
  def self.internal_reflected; Category.__send__(:find_by_name, "found"); end
end
"#);
        review_dynamic_finder_parity(&[
            ("app/models/application_record.rb", &parent),
            ("app/models/concerns/finder_hooks.rb", &concern),
            ("app/models/category.rb", &category),
        ], r#"
Category.create!(name: "found")
raise "bare dispatch" unless Category.bare == "owned"
raise "self dispatch" unless Category.explicit == "owned"
raise "reflected dispatch" unless Category.reflected == "owned"
raise "public reflected dispatch" unless Category.public_reflected == "owned"
raise "internal reflected dispatch" unless Category.internal_reflected == "owned"
"#);
    }
    review_dynamic_finder_parity(&[
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\nend\n"),
        ("app/models/category.rb", "class Category < ApplicationRecord\nend\n"),
        ("app/models/forum/category.rb", r#"module Forum
  class Category < ApplicationRecord
    self.table_name = "categories"
    def self.find_by_name(value); "namespaced owner"; end
  end
end
"#),
        ("app/models/forum/probe.rb", r#"module Forum
  class Probe < ApplicationRecord
    self.table_name = "categories"
    def self.lookup; Category.find_by_name("found"); end
    def self.reflected; Category.public_send(:find_by_name, "found"); end
  end
end
"#),
    ], r#"
raise "lexical owner" unless Forum::Probe.lookup == "namespaced owner"
raise "reflected lexical owner" unless Forum::Probe.reflected == "namespaced owner"
"#);
    review_dynamic_finder_parity(&[
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\nend\n"),
        ("app/models/category.rb", r#"class Category < ApplicationRecord
  scope :find_by_name, ->(value) { where(name: value) }
  def self.bare; find_by_name("found"); end
  def self.explicit; self.find_by_name("found"); end
  def self.reflected; send(:find_by_name, "found"); end
end
"#),
    ], r#"
Category.create!(name: "found")
Category.create!(name: "found")
raise "scope owns bare finder" unless Category.bare.to_a.length == 2
raise "scope owns self finder" unless Category.explicit.to_a.length == 2
raise "scope owns reflected finder" unless Category.reflected.to_a.length == 2
"#);
}

#[test]
fn class_dynamic_finder_review_preserves_scoped_helper_filters() {
    review_dynamic_finder_parity(&[
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\nend\n"),
        ("app/models/category.rb", r#"class Category < ApplicationRecord
  scope :active_only, -> { where(active: true) }
  def self.dynamic(id); find_by_id(id); end
  def self.explicit(id); self.find_by_id(id); end
  def self.bang(id); find_by_id!(id); end
  def self.reflected(id); public_send(:find_by_id, id); end
  def self.string_reflected(id); __send__("find_by_id", id); end
  def self.named_query(name); Category.active_only.find_by_name(name); end
end
"#),
        ("app/models/finder_probe.rb", r#"class FinderProbe
  def self.scoped(id); Category.active_only.dynamic(id); end
  def self.explicit(id); Category.active_only.explicit(id); end
  def self.reflected(id); Category.active_only.reflected(id); end
  def self.string_reflected(id); Category.active_only.string_reflected(id); end
  def self.relation(id); Category.where(active: true).dynamic(id); end
  def self.bang(id); Category.active_only.bang(id); end
end
"#),
    ], r#"
visible = Category.create!(name: "visible", active: true)
hidden = Category.create!(name: "hidden", active: false)
raise "scope hit" unless FinderProbe.scoped(visible.id).name == "visible"
raise "scope lost" unless FinderProbe.scoped(hidden.id).nil?
raise "self scope lost" unless FinderProbe.explicit(hidden.id).nil?
raise "reflection scope lost" unless FinderProbe.reflected(hidden.id).nil?
raise "string reflection scope lost" unless FinderProbe.string_reflected(hidden.id).nil?
raise "relation scope lost" unless FinderProbe.relation(hidden.id).nil?
raise "scoped direct finder" unless Category.named_query("hidden").nil?
begin
  FinderProbe.bang(hidden.id)
  raise "bang scope lost"
rescue ActiveRecord::RecordNotFound
end
"#);
}

#[test]
fn class_dynamic_finder_review_lowers_literal_reflection_spellings() {
    let mut methods = String::new();
    let mut service_methods = String::from("def self.direct; Category.find_by_name(\"found\"); end\n");
    let mut assertions = String::from("Category.create!(name: \"found\")\n");
    assertions.push_str("raise \"service direct\" unless FinderProbe.direct.name == \"found\"\n");
    for alias in ["send", "public_send", "__send__"] {
        for (receiver, suffix) in [("", "bare"), ("self.", "self"), ("Category.", "class"), ("::Category.", "absolute")] {
            let name = format!("{alias}_{suffix}");
            methods.push_str(&format!("def self.{name}; {receiver}{alias}(:find_by_name, \"found\"); end\n"));
            assertions.push_str(&format!("raise {name:?} unless Category.{name}.name == \"found\"\n"));
            methods.push_str(&format!("def self.{name}_bang; {receiver}{alias}(:find_by_name!, \"absent\"); end\n"));
            assertions.push_str(&format!("begin; Category.{name}_bang; raise \"bang reflection\"; rescue ActiveRecord::RecordNotFound; end\n"));
            methods.push_str(&format!("def self.{name}_string; {receiver}{alias}(\"find_by_name\", \"found\"); end\n"));
            assertions.push_str(&format!("raise \"string reflection\" unless Category.{name}_string.name == \"found\"\n"));
        }
        service_methods.push_str(&format!("def self.reflected_{alias}; Category.{alias}(:find_by_name, \"found\"); end\n"));
        assertions.push_str(&format!("raise \"service reflection\" unless FinderProbe.reflected_{alias}.name == \"found\"\n"));
        service_methods.push_str(&format!("def self.string_{alias}; Category.{alias}(\"find_by_name\", \"found\"); end\n"));
        assertions.push_str(&format!("raise \"service string reflection\" unless FinderProbe.string_{alias}.name == \"found\"\n"));
    }
    let category = format!("class Category < ApplicationRecord\n{methods}end\n");
    let service = format!("class FinderProbe\n{service_methods}end\n");
    review_dynamic_finder_parity(&[
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\nend\n"),
        ("app/models/category.rb", &category),
        ("app/models/finder_probe.rb", &service),
    ], &assertions);
    for alias in ["send", "public_send", "__send__"] {
        for finder in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
            let category = format!("class Category < ApplicationRecord\ndef self.{alias}(name, *args); \"owned wrapper\"; end\ndef self.probe; self.{alias}(\"{finder}\", \"found\"); end\nend\n");
            review_dynamic_finder_parity(&[
                ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\nend\n"),
                ("app/models/category.rb", &category),
            ], "raise \"wrapper dispatch\" unless Category.probe == \"owned wrapper\"\n");
        }
    }
}

#[test]
fn class_dynamic_finder_review_respects_source_delegate_results() {
    for (owner, delegate, value) in [
        ("own", "find_by", "\"delegated\""),
        ("parent", "find_by", "\"delegated\""),
        ("concern", "find_by", "\"delegated\""),
        ("own", "find_by!", "\"bang delegated\""),
        ("parent", "find_by!", "\"bang delegated\""),
        ("concern", "find_by!", "\"bang delegated\""),
        ("own", "find_by!", "nil"),
        ("parent", "find_by!", "nil"),
        ("concern", "find_by!", "nil"),
    ] {
        let definition = format!("def self.{delegate}(attributes); {value}; end\n");
        let parent = format!("class ApplicationRecord < ActiveRecord::Base\nself.abstract_class = true\n{}end\n", if owner == "parent" { &definition } else { "" });
        let concern = format!("module FinderHooks\nextend ActiveSupport::Concern\nclass_methods do\n{}end\nend\n", if owner == "concern" { definition.replace("def self.", "def ") } else { String::new() });
        let bang = if delegate == "find_by!" { "!" } else { "" };
        let category = format!(r#"class Category < ApplicationRecord
  {}
  def self.bare; find_by_name{bang}("found"); end
  def self.explicit; self.find_by_name{bang}("found"); end
  def self.raising; find_by_name!("found"); end
  def self.reflected; send(:find_by_name{bang} , "found"); end
end
"#, match owner { "own" => definition.as_str(), "concern" => "include FinderHooks", _ => "" });
        let assertions = format!("raise \"bare delegate\" unless Category.bare == {value}\nraise \"self delegate\" unless Category.explicit == {value}\nraise \"bang delegate\" unless Category.raising == {value}\n");
        let assertions = format!("{assertions}raise \"reflected delegate\" unless Category.reflected == {value}\n");
        let service = format!(r#"class FinderProbe
  def self.direct; Category.find_by_name{bang}("found"); end
  def self.bang; Category.find_by_name!("found"); end
  def self.reflected; Category.public_send(:find_by_name{bang}, "found"); end
end
"#);
        let assertions = format!("{assertions}raise \"service delegate\" unless FinderProbe.direct == {value}\nraise \"service bang delegate\" unless FinderProbe.bang == {value}\nraise \"service reflected delegate\" unless FinderProbe.reflected == {value}\n");
        review_dynamic_finder_parity(&[
            ("app/models/application_record.rb", &parent),
            ("app/models/concerns/finder_hooks.rb", &concern),
            ("app/models/category.rb", &category),
            ("app/models/finder_probe.rb", &service),
        ], &assertions);
    }
}

#[test]
fn class_dynamic_finder_review_distinguishes_class_values_from_record_locals() {
    for method in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
        let argument = if method == "find_by" || method == "find_by!" { "name: \"found\"" } else { "\"found\"" };
        let mut methods = String::new();
        let mut assertions = String::from("Category.create!(name: \"found\")\n");
        for (label, invoke) in [
            ("direct", format!("{method}({argument})")),
            ("send", format!("send(:{method}, {argument})")),
            ("public_send", format!("public_send(\"{method}\", {argument})")),
            ("internal", format!("__send__(:{method}, {argument})")),
        ] {
            for (kind, setup, expected) in [
                ("instance", "record = Category.new", "\"instance finder\""),
                ("instance_copy", "original = Category.new; record = original", "\"instance finder\""),
                ("reassigned", "record = Category; record = Category.new", "\"instance finder\""),
                ("class_copy", "klass = Category; record = klass", "\"found\""),
            ] {
                let result = if kind == "class_copy" { ".name" } else { "" };
                methods.push_str(&format!("def self.{kind}_{label}; {setup}; record.{invoke}{result}; end\n"));
                assertions.push_str(&format!("raise \"{method}/{kind}/{label}\" unless FinderProbe.{kind}_{label} == {expected}\n"));
            }
        }
        let category = format!(r#"class Category < ActiveRecord::Base
  def {method}(value); "instance finder"; end
  def local_lookup; record = self; record.{method}({argument}); end
  def explicit_lookup; self.{method}({argument}); end
end
"#);
        assertions.push_str("raise \"instance self\" unless Category.new.explicit_lookup == \"instance finder\"\nraise \"copied instance self\" unless Category.new.local_lookup == \"instance finder\"\n");
        review_dynamic_finder_parity(&[
            ("app/models/category.rb", &category),
            ("app/models/finder_probe.rb", &format!("class FinderProbe\n{methods}end\n")),
        ], &assertions);
    }
}

#[test]
fn class_dynamic_finder_review_keeps_polymorphic_class_dispatch() {
    for method in ["find_by_name", "find_by_name!"] {
        let category = format!(r#"class Category < ActiveRecord::Base
  def self.bare; {method}("found"); end
  def self.explicit; self.{method}("found"); end
end
"#);
        let special = format!(r#"class SpecialCategory < Category
  def self.{method}(value); "subclass finder"; end
end
"#);
        let service = format!(r#"class FinderProbe
  def self.returned_class; Category; end
  def self.returned; returned_class.{method}("found"); end
  def self.explicit_returned; FinderProbe.returned_class.{method}("found"); end
  def self.union(flag)
    klass = flag ? Category : SpecialCategory
    klass.{method}("found")
  end
  def self.reflected_union(flag)
    klass = flag ? Category : SpecialCategory
    klass.public_send(:{method}, "found")
  end
end
"#);
        review_dynamic_finder_parity(&[
            ("app/models/category.rb", &category),
            ("app/models/special_category.rb", &special),
            ("app/models/finder_probe.rb", &service),
        ], r#"
Category.create!(name: "found")
raise "base bare" unless Category.bare.name == "found"
raise "base self" unless Category.explicit.name == "found"
raise "subclass bare" unless SpecialCategory.bare == "subclass finder"
raise "subclass self" unless SpecialCategory.explicit == "subclass finder"
raise "returned class" unless FinderProbe.returned.name == "found"
raise "explicit returned class" unless FinderProbe.explicit_returned.name == "found"
raise "union framework" unless FinderProbe.union(true).name == "found"
raise "union source" unless FinderProbe.union(false) == "subclass finder"
raise "reflected union framework" unless FinderProbe.reflected_union(true).name == "found"
raise "reflected union source" unless FinderProbe.reflected_union(false) == "subclass finder"
"#);
    }
}

#[test]
fn class_dynamic_finder_review_preserves_subclass_identity_and_exclusions() {
    for with_type in [false, true] {
        for method in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
            let arg = if method == "find_by" || method == "find_by!" { "name: value" } else { "value" };
            let category = format!(r#"class Category < ActiveRecord::Base
  def self.lookup(value); {method}({arg}); end
end
"#);
            let hit = if method == "find_by" || method == "find_by!" { "name: \"child\"" } else { "\"child\"" };
            let excluded = if with_type {
                if method.ends_with('!') {
                    "begin; SpecialCategory.lookup(\"found\"); raise \"wrong STI row\"; rescue ActiveRecord::RecordNotFound; end".to_owned()
                } else {
                    "raise \"wrong STI row\" unless SpecialCategory.lookup(\"found\").nil?".to_owned()
                }
            } else {
                "raise \"plain subclass helper\" unless SpecialCategory.lookup(\"found\").class == SpecialCategory".to_owned()
            };
            let assertions = format!(r#"
Category.create!(name: "found")
SpecialCategory.create!(name: "child")
raise "subclass hit" unless FinderProbe.direct.class == SpecialCategory
raise "inherited subclass helper" unless SpecialCategory.lookup("child").class == SpecialCategory
raise "base class" unless Category.lookup("found").class == Category
{excluded}
"#);
            review_dynamic_finder_parity_schema(&[
                ("app/models/category.rb", &category),
                ("app/models/special_category.rb", "class SpecialCategory < Category\nend\n"),
                ("app/models/finder_probe.rb", &format!("class FinderProbe\n  def self.direct; SpecialCategory.{method}({hit}); end\nend\n")),
            ], &assertions, with_type);
        }
    }
}

#[test]
fn class_dynamic_finder_review_keeps_private_inherited_dispatch() {
    review_dynamic_finder_parity(&[
        ("app/models/category.rb", r#"class Category < ActiveRecord::Base
  def self.lookup; find_by_name("found"); end
  def self.explicit_lookup; self.find_by_name("found"); end
end
"#),
        ("app/models/special_category.rb", r#"class SpecialCategory < Category
  def self.find_by_name(value); "private finder"; end
  private_class_method :find_by_name
end
"#),
    ], r#"
Category.create!(name: "found")
raise "base" unless Category.lookup.name == "found"
raise "private bare" unless SpecialCategory.lookup == "private finder"
raise "private self" unless SpecialCategory.explicit_lookup == "private finder"
"#);
}

#[test]
fn class_dynamic_finder_review_evaluates_association_owner_once_before_arguments() {
    for method in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
        let arg = if method == "find_by" || method == "find_by!" { "name: Category.argument(value)" } else { "Category.argument(value)" };
        let service = format!(r#"class FinderProbe
  def self.lookup(id, value)
    Category.find(Category.owner_id(id)).children.{method}({arg})
  end
end
"#);
        let excluded = if method.ends_with('!') {
            "begin; FinderProbe.lookup(owner.id, \"foreign\"); raise \"foreign row\"; rescue ActiveRecord::RecordNotFound; end"
        } else {
            "raise \"foreign row\" unless FinderProbe.lookup(owner.id, \"foreign\").nil?"
        };
        review_dynamic_finder_parity(&[
            ("app/models/category.rb", r#"class Category < ActiveRecord::Base
  has_many :children, class_name: "Category", foreign_key: "parent_id"
  def self.owner_id(id)
    @order_log = (@order_log || "") + "owner;"
    id
  end
  def self.argument(value)
    @order_log = (@order_log || "") + "argument;"
    value
  end
  def self.order_log; @order_log || ""; end
  def self.reset_log; @order_log = ""; end
end
"#),
            ("app/models/finder_probe.rb", &service),
        ], &format!(r#"
owner = Category.create!(name: "owner")
other = Category.create!(name: "other")
child = Category.create!(name: "child", parent_id: owner.id)
Category.create!(name: "foreign", parent_id: other.id)
Category.reset_log
raise "hit" unless FinderProbe.lookup(owner.id, "child").id == child.id
raise "owner and argument order" unless Category.order_log == "owner;argument;"
Category.reset_log
{excluded}
raise "excluded owner and argument order" unless Category.order_log == "owner;argument;"
"#));
    }
}

#[test]
fn class_dynamic_finder_review_refuses_unthreaded_relation_source_delegates() {
    let category = "class Category < ActiveRecord::Base\n  def self.find_by(attributes); \"delegated\"; end\nend\n";
    let probe = "class FinderProbe\n  def self.lookup; Category.where(active: true).find_by_name(\"found\"); end\nend\n";
    let schema = "ActiveRecord::Schema.define do\n  create_table :categories do |t|\n    t.string :name\n    t.boolean :active\n  end\nend\n";
    let native = format!("gem \"activerecord\", \"8.1.4\"\nrequire \"active_record\"\nrequire \"sqlite3\"\nActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\nActiveRecord::Schema.verbose = false\n{schema}{category}{probe}puts FinderProbe.lookup\n");
    let ruby = std::env::var("PROBE_RUBY").unwrap_or_else(|_| "ruby".to_owned());
    let oracle = std::process::Command::new(ruby).args(["-e", &native]).output().unwrap();
    assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
    assert_eq!(String::from_utf8_lossy(&oracle.stdout).trim(), "delegated");
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", schema)
        .write("app/models/category.rb", category)
        .write("app/models/finder_probe.rb", probe)
        .run_ruby("");
    assert!(run.errors.iter().any(|e| e.contains("relation dynamic finder with source delegate")), "{:?}", run.errors);
}

#[test]
fn class_dynamic_finder_review_passes_a_positional_hash_to_source_delegates() {
    for (delegate, finder) in [("find_by", "find_by_name"), ("find_by!", "find_by_name!")] {
        let category = format!("class Category < ActiveRecord::Base\n  def self.{delegate}(name:); name; end\n  def self.lookup; {finder}(\"found\"); end\n  def self.reflected; send(:{finder}, \"found\"); end\nend\n");
        let assertions = format!(r#"
raise "ordinary keyword call" unless Category.{delegate}(name: "found") == "found"
begin
  Category.lookup
  raise "dynamic finder changed positional hash to keywords"
rescue ArgumentError
end
begin
  Category.reflected
  raise "reflected finder changed positional hash to keywords"
rescue ArgumentError
end
"#);
        review_dynamic_finder_parity(&[("app/models/category.rb", &category)], &assertions);
    }
}

#[test]
fn class_dynamic_finder_review_refuses_overridden_class_equality() {
    for method in ["find_by_name", "find_by_name!", "find_by", "find_by!"] {
        let arg = if method == "find_by" || method == "find_by!" { "name: \"absent\"" } else { "\"absent\"" };
        let ordinary = method == "find_by" || method == "find_by!";
        let helper = if ordinary { format!(" def self.lookup; {method}({arg}); end\n") } else { String::new() };
        let category = format!("class Category < ActiveRecord::Base\n self.table_name = \"categories\"\n def self.==(other); other.equal?(self) || other.equal?(Category); end\n{helper}end\n");
        let special = format!("class SpecialCategory < Category\n def self.{method}(value); \"owned\"; end\nend\n");
        let probe = format!("class FinderProbe\n def self.choose(flag)\n klass = flag ? Category : SpecialCategory\n klass.{method}({arg})\n end\nend\n");
        let schema = "ActiveRecord::Schema.define do\n create_table :categories do |t|; t.string :name; end\nend\n";
        let calls = if ordinary { "[-> { Category.lookup }, -> { SpecialCategory.lookup }]" } else { "[-> { FinderProbe.choose(true) }, -> { FinderProbe.choose(false) }]" };
        let native = format!(r#"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{schema}{category}{special}{probe}
values = {calls}.map do |call|
  begin; call.call; rescue ActiveRecord::RecordNotFound; "missing"; end
end
puts values.inspect
"#);
        let ruby = std::env::var("PROBE_RUBY").unwrap_or_else(|_| "ruby".to_owned());
        let oracle = std::process::Command::new(ruby).args(["-e", &native]).output().unwrap();
        assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
        let expected = if method.ends_with('!') { "[\"missing\", \"owned\"]" } else { "[nil, \"owned\"]" };
        assert_eq!(String::from_utf8_lossy(&oracle.stdout).trim(), expected, "{method}");
        let errors = emit_and_run::empty_app()
            .write("db/schema.rb", schema)
            .write("app/models/category.rb", &category)
            .write("app/models/special_category.rb", &special)
            .write("app/models/finder_probe.rb", &probe)
            .run_ruby("")
            .errors.clone();
        assert!(errors.iter().any(|e| e.contains("polymorphic finder with class equality override")), "{method}: {errors:?}");
    }
}
